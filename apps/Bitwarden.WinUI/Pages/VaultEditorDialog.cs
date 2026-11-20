using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using System.Globalization;

namespace Bitwarden_WinUI.Pages;

/// <summary>A native editor whose draft is retained until a confirmed successful write.</summary>
internal sealed class VaultEditorDialog
{
    private readonly VaultEditorData _original;
    private readonly Func<VaultEditorData, CancellationToken, Task<VaultMutationResult>> _save;
    private readonly Action _onWriteCancellation;
    private readonly ContentDialog _dialog;
    private readonly ScrollViewer _scroll;
    private readonly StackPanel _form = new() { Spacing = 14, MinWidth = 340, MaxWidth = 640 };
    private readonly StackPanel _typeFields = new() { Spacing = 12 };
    private readonly StackPanel _uriRows = new() { Spacing = 10 };
    private readonly StackPanel _customRows = new() { Spacing = 12 };
    private readonly ComboBox _type = new() { Header = "条目类型", HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly TextBox _name = new() { Header = "名称", PlaceholderText = "条目名称" };
    private readonly ComboBox _folder = new() { Header = "文件夹", HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly CheckBox _favorite = new() { Content = "收藏" };
    private readonly CheckBox _reprompt = new() { Content = "查看此条目时重新验证主密码" };
    private readonly TextBox _notes = new() { Header = "备注", AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 90, MaxHeight = 220 };
    private readonly InfoBar _notice = new() { IsOpen = false, IsClosable = false, Severity = InfoBarSeverity.Error };
    private readonly ProgressBar _progress = new() { IsIndeterminate = true, Visibility = Visibility.Collapsed };
    private readonly Dictionary<string, InputField> _inputs = [];
    private readonly Dictionary<int, Dictionary<string, string>> _draftTypes = [];
    private readonly List<UriRow> _uris = [];
    private readonly List<CustomRow> _custom = [];
    private StackPanel _uriSection = null!;
    private CancellationTokenSource? _request;
    private bool _busy;
    private bool _aborted;
    private bool _ready;
    private bool _mustResync;
    private int _activeType;
    private VaultMutationResult? _result;

    private VaultEditorDialog(XamlRoot root, VaultEditorData data, VaultFolder[] folders,
        Func<VaultEditorData, CancellationToken, Task<VaultMutationResult>> save, Action onWriteCancellation)
    {
        _original = data;
        _save = save;
        _onWriteCancellation = onWriteCancellation;
        _activeType = data.ItemType;
        _draftTypes[data.ItemType] = new Dictionary<string, string>(data.Values);
        _scroll = new ScrollViewer { Content = _form, VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled, MaxHeight = Math.Max(220, Math.Min(620, root.Size.Height - 200)) };
        _dialog = new ContentDialog
        {
            XamlRoot = root, Title = data.Id is null ? "新增条目" : "编辑条目",
            PrimaryButtonText = "保存", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary,
            Content = _scroll,
        };
        _dialog.Resources["ContentDialogMaxWidth"] = 760d;
        AutomationProperties.SetAutomationId(_name, "VaultEditorName");
        AutomationProperties.SetAutomationId(_type, "VaultEditorType");
        AutomationProperties.SetAutomationId(_folder, "VaultEditorFolder");
        AutomationProperties.SetAutomationId(_notes, "VaultEditorNotes");
        AutomationProperties.SetAutomationId(_notice, "VaultEditorNotice");
        foreach (var type in Enumerable.Range(1, 8)) _type.Items.Add(new ComboBoxItem { Content = VaultTypes.Label(type), Tag = type });
        _type.SelectedIndex = Math.Clamp(data.ItemType, 1, 8) - 1;
        _type.IsEnabled = data.Id is null;
        _name.Text = data.Name;
        _notes.Text = data.Notes ?? string.Empty;
        _favorite.IsChecked = data.Favorite;
        _reprompt.IsChecked = data.Reprompt;
        _folder.Items.Add(new ComboBoxItem { Content = "无文件夹", Tag = string.Empty });
        foreach (var folder in folders) _folder.Items.Add(new ComboBoxItem { Content = folder.Name, Tag = folder.Id });
        if (data.FolderId is not null && !folders.Any(folder => folder.Id == data.FolderId))
            _folder.Items.Add(new ComboBoxItem { Content = "原文件夹（尚未加载）", Tag = data.FolderId });
        _folder.SelectedItem = _folder.Items.OfType<ComboBoxItem>().FirstOrDefault(item => Equals(item.Tag, data.FolderId ?? string.Empty));

        _form.Children.Add(_notice);
        if (data.Id is null) _form.Children.Add(new TextBlock { Text = "新条目将创建在个人保险库。", TextWrapping = TextWrapping.Wrap });
        _form.Children.Add(_type);
        _form.Children.Add(_name);
        _form.Children.Add(_folder);
        _form.Children.Add(_favorite);
        _form.Children.Add(_reprompt);
        _form.Children.Add(_typeFields);
        _uriSection = Section("网站 / URI", _uriRows, "添加网站", () => AddUri(new VaultUri(string.Empty, null)));
        _form.Children.Add(_uriSection);
        _form.Children.Add(_notes);
        _form.Children.Add(Section("自定义字段", _customRows, "添加自定义字段", () => AddCustom(new VaultCustomField(string.Empty, string.Empty, 0, null))));
        _form.Children.Add(_progress);
        BuildTypeFields(data.ItemType);
        foreach (var uri in data.Uris) AddUri(uri);
        if (data.ItemType == 1 && data.Uris.Length == 0) AddUri(new VaultUri(string.Empty, null));
        foreach (var field in data.CustomFields) AddCustom(field);
        _ready = true;
        _type.SelectionChanged += (_, _) =>
        {
            if (!_ready || _busy || _type.SelectedItem is not ComboBoxItem { Tag: int kind }) return;
            _draftTypes[_activeType] = ReadValues();
            _activeType = kind;
            BuildTypeFields(kind);
            foreach (var field in _custom) field.RefreshLinkedOptions(kind);
        };
        _dialog.PrimaryButtonClick += Save_Click;
        _dialog.CloseButtonClick += (_, args) => { if (_busy) { args.Cancel = true; CancelWrite(); } };
        _dialog.Closing += (_, args) => { if (_busy && !_aborted) { args.Cancel = true; CancelWrite(); } };
    }

    internal static VaultEditorDialog Create(XamlRoot root, VaultEditorData data, VaultFolder[] folders,
        Func<VaultEditorData, CancellationToken, Task<VaultMutationResult>> save, Action onWriteCancellation) => new(root, data, folders, save, onWriteCancellation);

    internal async Task<VaultMutationResult?> ShowAsync(CancellationToken cancellationToken)
    {
        using var registration = cancellationToken.Register(() => _dialog.DispatcherQueue.TryEnqueue(() =>
        {
            // Cancellation while writing must wait for the native outcome, which may be an acknowledged success.
            if (_busy) CancelWrite();
            else Abort();
        }));
        try { await _dialog.ShowAsync(); return _result; }
        finally
        {
            _aborted = true;
            _request?.Cancel();
            _request?.Dispose();
            _request = null;
            _name.Text = _notes.Text = string.Empty;
            foreach (var input in _inputs.Values) input.Clear();
            foreach (var uri in _uris) uri.Clear();
            foreach (var field in _custom) field.Clear();
            foreach (var values in _draftTypes.Values) values.Clear();
        }
    }

    internal void Abort() { if (_busy) _onWriteCancellation(); _aborted = true; _request?.Cancel(); _dialog.Hide(); }
    private void CancelWrite() { _onWriteCancellation(); _request?.Cancel(); }

    private async void Save_Click(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        args.Cancel = true;
        if (_busy || _mustResync || _aborted) return;
        if (string.IsNullOrWhiteSpace(_name.Text)) { Error("请输入条目名称。"); _name.Focus(FocusState.Programmatic); return; }
        foreach (var schema in Schemas.For(_activeType).Where(schema => schema.Date))
        {
            var value = _inputs[schema.Key].Read();
            if (value.Length > 0 && !DateOnly.TryParseExact(value, "yyyy-MM-dd", CultureInfo.InvariantCulture, DateTimeStyles.None, out _))
            { Error($"{schema.Label}请使用 yyyy-MM-dd 格式。"); return; }
        }
        VaultEditorData draft;
        try
        {
            draft = _original with
            {
                Name = _name.Text, ItemType = _activeType, FolderId = (_folder.SelectedItem as ComboBoxItem)?.Tag is string folder && folder.Length > 0 ? folder : null,
                Favorite = _favorite.IsChecked == true, Reprompt = _reprompt.IsChecked == true, Notes = _original.Notes is null && _notes.Text.Length == 0 ? null : _notes.Text,
                Values = ReadValues(), Uris = _activeType == 1 ? _uris.Select(row => row.Read()).ToArray() : [],
                CustomFields = _custom.Select(row => row.Read()).ToArray(),
            };
        }
        catch (InvalidOperationException error) { Error(error.Message); return; }
        var deferral = args.GetDeferral();
        _request?.Dispose();
        var request = _request = new CancellationTokenSource();
        SetBusy(true);
        _notice.IsOpen = false;
        try
        {
            var result = await _save(draft, request.Token);
            _result = result;
            if (_aborted) return;
            SetBusy(false);
            _dialog.Hide();
        }
        catch (OperationCanceledException) { _mustResync = true; if (!_aborted) Error("请求已取消。草稿仍保留，请先同步核对服务器状态，避免重复创建或覆盖。"); }
        catch (VaultException error)
        {
            if (_aborted) return;
            _mustResync = error.Code is "conflict" or "writeUncertain" or "notSynced";
            Error(error.UserMessage + (_mustResync ? " 当前表单仍保留草稿。请取消编辑并同步核对，再重新编辑；不要重复提交。" : string.Empty));
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        { if (!_aborted) Error("Rust 核心未能加载，请修复应用安装。当前草稿仍保留。"); }
        finally
        {
            if (ReferenceEquals(_request, request)) _request = null;
            request.Dispose();
            if (!_aborted) SetBusy(false);
            deferral.Complete();
        }
    }

    private void SetBusy(bool busy)
    {
        _busy = busy;
        _scroll.IsEnabled = !busy;
        _progress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        _dialog.IsPrimaryButtonEnabled = !busy && !_mustResync;
        _dialog.CloseButtonText = busy ? "取消请求" : "取消";
    }
    private void Error(string message) { _notice.Message = message; _notice.IsOpen = true; }
    private Dictionary<string, string> ReadValues()
    {
        var values = _draftTypes.TryGetValue(_activeType, out var previous) ? new Dictionary<string, string>(previous) : [];
        foreach (var (key, input) in _inputs)
        {
            var value = input.Read();
            if (value.Length > 0 || values.ContainsKey(key)) values[key] = value;
        }
        return values;
    }
    private void BuildTypeFields(int kind)
    {
        foreach (var input in _inputs.Values) input.Clear();
        _inputs.Clear();
        _typeFields.Children.Clear();
        var values = _draftTypes.GetValueOrDefault(kind) ?? [];
        foreach (var schema in Schemas.For(kind))
        {
            var input = new InputField(schema.Label, values.GetValueOrDefault(schema.Key) ?? string.Empty, schema.Secret, schema.Multiline, schema.Date);
            AutomationProperties.SetAutomationId(input.Element, $"VaultEditor_{schema.Key}");
            _inputs[schema.Key] = input;
            _typeFields.Children.Add(input.Element);
        }
        _uriSection.Visibility = kind == 1 ? Visibility.Visible : Visibility.Collapsed;
    }
    private void AddUri(VaultUri value)
    {
        UriRow? row = null;
        row = new UriRow(value, () => { _uriRows.Children.Remove(row!.Element); _uris.Remove(row); row.Clear(); });
        _uris.Add(row); _uriRows.Children.Add(row.Element);
    }
    private void AddCustom(VaultCustomField value)
    {
        CustomRow? row = null;
        row = new CustomRow(value, _activeType, () => { _customRows.Children.Remove(row!.Element); _custom.Remove(row); row.Clear(); });
        _custom.Add(row); _customRows.Children.Add(row.Element);
    }
    private static StackPanel Section(string title, StackPanel rows, string addLabel, Action add)
    {
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(new TextBlock { Text = title, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
        panel.Children.Add(rows);
        var button = new Button { Content = addLabel, HorizontalAlignment = HorizontalAlignment.Left };
        button.Click += (_, _) => add(); panel.Children.Add(button); return panel;
    }

    private sealed class InputField
    {
        internal FrameworkElement Element { get; }
        private readonly TextBox? _text;
        private readonly PasswordBox? _password;
        private readonly ToggleSwitch? _reveal;
        internal InputField(string label, string value, bool secret, bool multiline, bool date = false)
        {
            if (!secret)
            {
                _text = new TextBox { Header = label, Text = value, AcceptsReturn = multiline, TextWrapping = multiline ? TextWrapping.Wrap : TextWrapping.NoWrap,
                    MinHeight = multiline ? 100 : 0, MaxHeight = multiline ? 240 : double.PositiveInfinity, PlaceholderText = date ? "yyyy-MM-dd" : string.Empty };
                Element = _text; return;
            }
            _password = new PasswordBox { Header = label, Password = value, PasswordRevealMode = PasswordRevealMode.Peek };
            if (!multiline) { Element = PasswordVisibility.WithToggle(_password); return; }
            var panel = new StackPanel { Spacing = 6 };
            _text = new TextBox { Header = label, AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 120, MaxHeight = 260, Visibility = Visibility.Collapsed };
            _reveal = new ToggleSwitch { Header = $"明文编辑{label}", IsOn = false, OnContent = "显示", OffContent = "隐藏" };
            _reveal.Toggled += (_, _) =>
            {
                if (_reveal.IsOn) { _text.Text = _password.Password; _text.Visibility = Visibility.Visible; _password.Visibility = Visibility.Collapsed; }
                else { _password.Password = _text.Text; _text.Text = string.Empty; _text.Visibility = Visibility.Collapsed; _password.Visibility = Visibility.Visible; }
            };
            panel.Children.Add(_password); panel.Children.Add(_text); panel.Children.Add(_reveal); Element = panel;
        }
        internal string Read() => _password is null || _reveal?.IsOn == true ? _text?.Text ?? string.Empty : _password.Password;
        internal void Clear() { if (_text is not null) _text.Text = string.Empty; if (_password is not null) _password.Password = string.Empty; }
    }

    private sealed class UriRow
    {
        internal StackPanel Element { get; } = new() { Spacing = 6 };
        private readonly TextBox _uri = new() { Header = "网站 / URI", PlaceholderText = "https://example.com" };
        private readonly ComboBox _match = new() { Header = "匹配方式", HorizontalAlignment = HorizontalAlignment.Stretch };
        private readonly string? _initialUri;
        internal UriRow(VaultUri value, Action remove)
        {
            _initialUri = value.Uri; _uri.Text = value.Uri ?? string.Empty;
            foreach (var (label, match) in new (string, int?)[] { ("默认", null), ("域名", 0), ("主机", 1), ("开头匹配", 2), ("完全匹配", 3), ("正则表达式", 4), ("从不", 5) })
                _match.Items.Add(new ComboBoxItem { Content = label, Tag = match });
            _match.SelectedItem = _match.Items.OfType<ComboBoxItem>().FirstOrDefault(item => Equals(item.Tag, value.Match));
            var button = new Button { Content = "移除此网站", HorizontalAlignment = HorizontalAlignment.Left };
            button.Click += (_, _) => remove();
            Element.Children.Add(_uri); Element.Children.Add(_match); Element.Children.Add(button);
        }
        internal VaultUri Read() => new(_initialUri is null && _uri.Text.Length == 0 ? null : _uri.Text, (_match.SelectedItem as ComboBoxItem)?.Tag as int?);
        internal void Clear() => _uri.Text = string.Empty;
    }

    private sealed class CustomRow
    {
        internal StackPanel Element { get; } = new() { Spacing = 8 };
        private readonly TextBox _name = new() { Header = "字段名称" };
        private readonly ComboBox _kind = new() { Header = "字段类型", HorizontalAlignment = HorizontalAlignment.Stretch };
        private readonly TextBox _text = new() { Header = "值" };
        private readonly PasswordBox _hidden = new() { Header = "隐藏值", PasswordRevealMode = PasswordRevealMode.Peek };
        private readonly CheckBox _boolean = new() { Content = "值为 true" };
        private readonly ComboBox _linked = new() { Header = "关联目标", HorizontalAlignment = HorizontalAlignment.Stretch };
        private bool _ready;
        private int _previousType;
        private readonly VaultCustomField _initial;
        internal CustomRow(VaultCustomField value, int itemType, Action remove)
        {
            _initial = value; _previousType = value.Type;
            _name.Text = value.Name ?? string.Empty;
            _text.Text = value.Type == 0 ? value.Value ?? string.Empty : string.Empty;
            _hidden.Password = value.Type == 1 ? value.Value ?? string.Empty : string.Empty;
            _boolean.IsChecked = string.Equals(value.Value, "true", StringComparison.OrdinalIgnoreCase);
            foreach (var (label, kind) in new[] { ("文本", 0), ("隐藏", 1), ("布尔值", 2), ("关联", 3) })
                _kind.Items.Add(new ComboBoxItem { Content = label, Tag = kind });
            _kind.SelectedIndex = Math.Clamp(value.Type, 0, 3);
            RefreshLinkedOptions(itemType, value.LinkedId);
            var removeButton = new Button { Content = "移除此字段", HorizontalAlignment = HorizontalAlignment.Left };
            removeButton.Click += (_, _) => remove();
            foreach (var element in new UIElement[] { _name, _kind, _text, _hidden, _boolean, _linked, removeButton }) Element.Children.Add(element);
            RefreshVisibility(); _ready = true;
            _kind.SelectionChanged += (_, _) =>
            {
                if (!_ready) return;
                var current = _previousType == 1 ? _hidden.Password : _previousType == 2 ? (_boolean.IsChecked == true ? "true" : "false") : _text.Text;
                var type = _kind.SelectedIndex;
                if (type == 0) _text.Text = current;
                if (type == 1) _hidden.Password = current;
                if (type == 2) _boolean.IsChecked = string.Equals(current, "true", StringComparison.OrdinalIgnoreCase);
                _previousType = type; RefreshVisibility();
            };
        }
        internal void RefreshLinkedOptions(int itemType, int? selected = null)
        {
            selected ??= (_linked.SelectedItem as ComboBoxItem)?.Tag as int?;
            _linked.Items.Clear();
            foreach (var (label, id) in Schemas.Links(itemType)) _linked.Items.Add(new ComboBoxItem { Content = label, Tag = id });
            if (selected is int existing && !_linked.Items.OfType<ComboBoxItem>().Any(item => Equals(item.Tag, existing)))
                _linked.Items.Add(new ComboBoxItem { Content = $"原关联字段 ({existing})", Tag = existing });
            _linked.SelectedItem = _linked.Items.OfType<ComboBoxItem>().FirstOrDefault(item => Equals(item.Tag, selected)) ?? _linked.Items.FirstOrDefault();
            if (_kind.Items.Count > 3 && _kind.Items[3] is ComboBoxItem linked) linked.IsEnabled = _linked.Items.Count > 0;
        }
        private void RefreshVisibility()
        {
            _text.Visibility = _kind.SelectedIndex == 0 ? Visibility.Visible : Visibility.Collapsed;
            _hidden.Visibility = _kind.SelectedIndex == 1 ? Visibility.Visible : Visibility.Collapsed;
            _boolean.Visibility = _kind.SelectedIndex == 2 ? Visibility.Visible : Visibility.Collapsed;
            _linked.Visibility = _kind.SelectedIndex == 3 ? Visibility.Visible : Visibility.Collapsed;
            // Do not leave a hidden value mirrored in a collapsed, unprotected text control.
            if (_kind.SelectedIndex != 0) _text.Text = string.Empty;
            if (_kind.SelectedIndex != 1) _hidden.Password = string.Empty;
            if (_kind.SelectedIndex != 2) _boolean.IsChecked = false;
        }
        internal VaultCustomField Read()
        {
            var kind = _kind.SelectedIndex;
            var linked = (_linked.SelectedItem as ComboBoxItem)?.Tag as int?;
            if (kind == 3 && linked is null) throw new InvalidOperationException("请选择自定义字段的关联目标。");
            var value = kind switch { 1 => _hidden.Password, 2 => _boolean.IsChecked == true ? "true" : "false", 3 => null, _ => _text.Text };
            if (kind == _initial.Type && _initial.Value is null && value?.Length == 0) value = null;
            return new VaultCustomField(_initial.Name is null && _name.Text.Length == 0 ? null : _name.Text, value, kind, kind == 3 ? linked : null);
        }
        internal void Clear() { _name.Text = _text.Text = _hidden.Password = string.Empty; }
    }

    private sealed record FieldSchema(string Key, string Label, bool Secret = false, bool Multiline = false, bool Date = false);
    private static class Schemas
    {
        internal static FieldSchema[] For(int type) => type switch
        {
            1 => [new("username", "用户名"), new("password", "密码", true), new("totp", "验证器密钥（TOTP）", true)],
            2 => [],
            3 => [new("cardholderName", "持卡人"), new("number", "卡号", true), new("brand", "卡组织"), new("expMonth", "到期月份"), new("expYear", "到期年份"), new("code", "安全码", true)],
            4 => [new("title", "称谓"), new("firstName", "名"), new("middleName", "中间名"), new("lastName", "姓"), new("username", "用户名"), new("company", "公司"), new("email", "邮箱"), new("phone", "电话"), new("address1", "地址 1"), new("address2", "地址 2"), new("address3", "地址 3"), new("city", "城市"), new("state", "省 / 州"), new("postalCode", "邮编"), new("country", "国家"), new("ssn", "社会保障号码", true), new("passportNumber", "护照号码", true), new("licenseNumber", "驾照号码", true)],
            5 => [new("privateKey", "SSH 私钥", true, true), new("publicKey", "SSH 公钥", false, true), new("fingerprint", "指纹")],
            6 => [new("bankName", "银行名称"), new("nameOnAccount", "账户持有人"), new("accountType", "账户类型"), new("accountNumber", "账号", true), new("routingNumber", "路由号码", true), new("branchNumber", "分行号码"), new("pin", "PIN", true), new("swiftCode", "SWIFT"), new("iban", "IBAN", true), new("bankContactPhone", "银行联系电话")],
            7 => [new("firstName", "名"), new("middleName", "中间名"), new("lastName", "姓"), new("dateOfBirth", "出生日期", false, false, true), new("licenseNumber", "驾照号码", true), new("issuingCountry", "签发国家"), new("issuingState", "签发省 / 州"), new("issueDate", "签发日期", false, false, true), new("expirationDate", "到期日期", false, false, true), new("issuingAuthority", "签发机构"), new("licenseClass", "准驾类型")],
            8 => [new("surname", "姓"), new("givenName", "名"), new("dateOfBirth", "出生日期", false, false, true), new("sex", "性别"), new("birthPlace", "出生地"), new("nationality", "国籍"), new("issuingCountry", "签发国家"), new("passportNumber", "护照号码", true), new("passportType", "护照类型"), new("nationalIdentificationNumber", "国民身份号码", true), new("issuingAuthority", "签发机构"), new("issueDate", "签发日期", false, false, true), new("expirationDate", "到期日期", false, false, true)],
            _ => [],
        };
        internal static (string, int)[] Links(int type) => type switch
        {
            1 => [("用户名", 100), ("密码", 101)],
            3 => [("持卡人", 300), ("到期月份", 301), ("到期年份", 302), ("安全码", 303), ("卡组织", 304), ("卡号", 305)],
            4 => [("称谓", 400), ("中间名", 401), ("地址 1", 402), ("地址 2", 403), ("地址 3", 404), ("城市", 405), ("省 / 州", 406), ("邮编", 407), ("国家", 408), ("公司", 409), ("邮箱", 410), ("电话", 411), ("社会保障号码", 412), ("用户名", 413), ("护照号码", 414), ("驾照号码", 415), ("名", 416), ("姓", 417), ("全名", 418)],
            _ => [],
        };
    }
}
