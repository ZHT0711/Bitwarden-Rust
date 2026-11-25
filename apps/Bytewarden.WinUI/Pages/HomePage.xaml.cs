using Bytewarden.WinUI.Interop;
using Bytewarden.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Bytewarden.WinUI.Pages;

public sealed partial class HomePage : Page
{
    internal sealed record FolderNavigation(string Id);

    public event EventHandler? SignInRequested;
    private VaultOverview? _overview;
    private string _category = "vault";
    private string _query = string.Empty;
    private string? _initialFolderId;
    private bool _loaded;
    private bool _syncing;
    private int _version;
    private int _displayedItemCount = -1;
    private VaultItem[]? _lastFilterSource;
    private string? _lastFilterCategory;
    private string? _lastFilterFolder;
    private string? _lastFilterQuery;
    private VaultItem[]? _lastFilteredItems;
    private CancellationTokenSource? _operation;
    private TotpCodeView? _totpView;

    public HomePage()
    {
        InitializeComponent();
        InitializeListOptions();
        PopulateFolders();
        Loaded += async (_, _) => { _loaded = true; await RefreshAsync(false); };
        Unloaded += (_, _) => { _loaded = false; ClearSensitiveState(); };
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        _category = e.Parameter as string ?? "vault";
        _initialFolderId = (e.Parameter as FolderNavigation)?.Id;
        UpdateCategoryHeader();
        base.OnNavigatedTo(e);
    }

    internal void SwitchCategory(string category, string? folderId = null)
    {
        if (!_loaded)
        {
            _category = category;
            _initialFolderId = folderId;
            UpdateCategoryHeader();
            return;
        }
        var targetCategory = folderId is not null ? "vault" : category;
        if (_category == targetCategory && folderId is null) return;
        _category = targetCategory;
        UpdateCategoryHeader();
        if (_selectingItems) SetItemSelectionMode(false);
        if (folderId is not null)
        {
            if (_folderChoices.TryGetValue(folderId, out var choice) && !ReferenceEquals(FolderFilter.SelectedItem, choice))
            {
                FolderFilter.SelectedItem = choice;
                return;
            }
            _initialFolderId = folderId;
        }
        ApplyFilters();
    }

    private void UpdateCategoryHeader()
    {
        (PageTitle.Text, PageSubtitle.Text) = _category switch
        {
            "favorites" => ("收藏夹", "你标记为收藏的保险库条目。"),
            "logins" => ("登录", "网站和应用的账号凭据。"),
            "notes" => ("安全笔记", "安全保存文本和重要信息。"),
            "cards" => ("银行卡", "管理支付卡片信息。"),
            "identities" => ("身份信息", "管理个人身份与地址信息。"),
            "ssh" => ("SSH 密钥", "管理 SSH 公钥与私钥。"),
            "bankaccounts" => ("银行账户", "银行账号与汇款信息。"),
            "licenses" => ("驾驶执照", "驾驶执照与签发信息。"),
            "passports" => ("护照", "护照与签发信息。"),
            "archived" => ("归档", "已归档的保险库条目。"),
            "trash" => ("回收站", "已删除的保险库条目。"),
            _ => ("我的保险库", "密码、笔记与敏感信息都集中在这里。"),
        };
    }

    public void SetSearch(string query)
    {
        query = query.Trim();
        if (_query == query) return;
        _query = query;
        ApplyFilters();
    }
    public void ShowAuthenticated() => _ = RefreshAsync(true);

    public void ShowLoggedOut()
    {
        ClearSensitiveState();
        SetNotice("账户已退出", "当前没有活跃的账户会话。", InfoBarSeverity.Informational);
        ConnectButton.Visibility = Visibility.Visible;
        ConnectButton.Content = "连接账户";
        EmptyMessage.Text = "登录以查看你的保险库";
    }

    internal void ShowLocked()
    {
        ClearSensitiveState();
        SetNotice("保险库已锁定", "解锁以继续使用当前账户。", InfoBarSeverity.Informational);
        ConnectButton.Visibility = Visibility.Visible;
        ConnectButton.Content = "解锁保险库";
        EmptyMessage.Text = "保险库已锁定";
    }

    public void ClearSensitiveState()
    {
        AbortMutationFlow();
        _version++;
        CancelOperation();
        _overview = null;
        _orderedSource = null;
        _orderedItems = [];
        ResetFilterCache();
        _displayedItemCount = -1;
        _initialFolderId = null;
        _editorOpen = false;
        _selectingItems = false;
        _requiresSync = false;
        _syncing = false;
        VaultList.ItemsSource = null;
        VaultList.SelectionMode = ListViewSelectionMode.Single;
        ClearDetailInputs(DetailPanel);
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock { Text = "选择一个条目查看详情" });
        PopulateFolders();
        ApplyFilters();
        SetBusy(false);
    }

    private void ResetFilterCache()
    {
        _lastFilterSource = null;
        _lastFilterCategory = null;
        _lastFilterFolder = null;
        _lastFilterQuery = null;
        _lastFilteredItems = null;
    }

    private static void ClearDetailInputs(Panel panel)
    {
        foreach (var child in panel.Children)
        {
            if (child is PasswordBox passwordBox) passwordBox.Password = string.Empty;
            else if (child is TextBox textBox) textBox.Text = string.Empty;
            else if (child is Panel nested) ClearDetailInputs(nested);
            else if (child is Border { Child: Panel borderedPanel }) ClearDetailInputs(borderedPanel);
            else if (child is Expander { Content: Panel expandedPanel }) ClearDetailInputs(expandedPanel);
        }
    }

    public async Task RefreshAsync(bool forceSync, bool allowSyncFallback = true)
    {
        if (!_loaded || _syncing || _editorOpen) return;
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        _syncing = true;
        SetBusy(true);
        DetailPanel.Children.Clear();
        VaultList.SelectedItem = null;
        try
        {
            var state = RustCore.SessionState;
            if (state == VaultSessionState.Locked) { ShowLocked(); return; }
            if (state != VaultSessionState.Unlocked) { ShowLoggedOut(); return; }
            ConnectButton.Visibility = Visibility.Collapsed;
            SetNotice("正在加载保险库", forceSync ? "正在同步并在本机解密…" : "正在读取本次会话的保险库…", InfoBarSeverity.Informational);
            VaultOverview result;
            try
            {
                result = forceSync ? await Task.Run(() => RustCore.SyncVault(token), token)
                    : await Task.Run(RustCore.GetVaultOverview, token);
            }
            catch (VaultException error) when (allowSyncFallback && !forceSync && (error.Code is "notFound" or "notSynced"))
            {
                result = await Task.Run(() => RustCore.SyncVault(token), token);
            }
            if (!IsCurrent(version, token)) return;
            _overview = result;
            _requiresSync = result.ReadOnly;
            PopulateFolders();
            ApplyFilters();
            var count = $"共 {result.Items.Length} 个条目；上次同步 {result.LastSync.ToLocalTime():MM-dd HH:mm:ss}。" +
                (result.Failures > 0 ? $" {result.Failures} 个条目或文件夹解析 / 解密失败。" : string.Empty);
            if (result.ReadOnly)
                SetNotice("本机保险库缓存（只读）", count + " 重新连接或成功同步后可编辑。", InfoBarSeverity.Informational);
            else
                SetNotice(result.Failures > 0 ? "保险库已加载，部分条目无法读取" : "保险库已加载",
                    count, result.Failures > 0 ? InfoBarSeverity.Warning : InfoBarSeverity.Success);
        }
        catch (OperationCanceledException)
        {
            if (_loaded && version == _version) SetNotice("请求已取消", "你可以重新同步。", InfoBarSeverity.Informational);
        }
        catch (VaultException error)
        {
            if (IsCurrent(version, token) && forceSync)
            {
                try
                {
                    var cached = await Task.Run(RustCore.GetVaultOverview, token);
                    if (!IsCurrent(version, token)) return;
                    _overview = cached;
                    _requiresSync |= cached.ReadOnly;
                    PopulateFolders();
                    ApplyFilters();
                    SetNotice("同步失败，显示上次同步的数据", error.UserMessage, InfoBarSeverity.Warning);
                    return;
                }
                catch (OperationCanceledException) { return; }
                catch (VaultException cacheError)
                {
                    if (!IsCurrent(version, token)) return;
                    if (cacheError.Code == "notSynced")
                    {
                        _overview = null;
                        _requiresSync = true;
                        PopulateFolders();
                    }
                }
            }
            if (IsCurrent(version, token)) SetNotice("无法加载保险库", error.UserMessage, InfoBarSeverity.Error);
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请重新构建或修复应用安装。", InfoBarSeverity.Error); }
        finally
        {
            if (version == _version) { _syncing = false; SetBusy(false); ApplyFilters(); }
        }
    }

    private void PopulateFolders()
    {
        var selected = _initialFolderId ?? (FolderFilter.SelectedItem as FolderChoice)?.Id;
        if (_overview is not null) _initialFolderId = null;
        _folderHierarchy = FolderHierarchy.Build(_overview?.Folders ?? [], _overview?.Items ?? []);
        var all = _folderHierarchy.All;
        var choices = new FolderChoice[all.Length + 2];
        choices[0] = new("*", "所有文件夹");
        choices[1] = new("", "无文件夹");
        var byId = new Dictionary<string, FolderChoice>(choices.Length, StringComparer.Ordinal)
        {
            [choices[0].Id] = choices[0],
            [choices[1].Id] = choices[1],
        };
        for (var index = 0; index < all.Length; index++)
        {
            var folder = all[index];
            var choice = new FolderChoice(folder.Id, folder.Name);
            choices[index + 2] = choice;
            byId[choice.Id] = choice;
        }
        _folderChoices = byId;
        FolderFilter.ItemsSource = choices;
        FolderFilter.SelectedItem = selected is not null && byId.TryGetValue(selected, out var existing)
            ? existing : choices[0];
    }

    private void ApplyFilters()
    {
        var folder = (FolderFilter.SelectedItem as FolderChoice)?.Id ?? "*";
        var ordered = OrderedItems();
        var canNarrow = ReferenceEquals(_lastFilterSource, ordered) &&
            _lastFilterCategory == _category &&
            _lastFilterFolder == folder &&
            _lastFilteredItems is not null &&
            _lastFilterQuery is not null &&
            !_lastFilterQuery.Contains('"') &&
            !_query.Contains('"') &&
            _query.StartsWith(_lastFilterQuery, StringComparison.OrdinalIgnoreCase);
        var source = canNarrow ? _lastFilteredItems! : ordered;
        var items = VaultQuery.Apply(source, _category, folder, _query);
        _lastFilterSource = ordered;
        _lastFilterCategory = _category;
        _lastFilterFolder = folder;
        _lastFilterQuery = _query;
        _lastFilteredItems = items;

        UpdateFolderNavigation(folder);
        var listChanged = UpdateListItems(items);
        if (_displayedItemCount != items.Length)
        {
            _displayedItemCount = items.Length;
            ItemCount.Text = $"{items.Length} 个条目";
        }
        var emptyVisibility = items.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        if (EmptyState.Visibility != emptyVisibility) EmptyState.Visibility = emptyVisibility;
        if (items.Length == 0)
        {
            var message = _overview is null ? "登录或同步以查看你的保险库" : _query.Length > 0 ? "没有匹配的条目" : "此分类或文件夹中没有条目";
            if (EmptyMessage.Text != message) EmptyMessage.Text = message;
        }
        if (listChanged || _overview is null || _syncing) UpdateMutationCommands();
    }

    private async void VaultList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_updatingList || _syncing || _editorOpen) return;
        UpdateListLayout();
        if (_selectingItems)
        {
            UpdateMutationCommands();
            return;
        }
        if (VaultList.SelectedItem is not VaultItem item)
        {
            CancelOperation();
            _version++;
            DetailPanel.Children.Clear();
            SetBusy(false);
            return;
        }
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        DetailPanel.Children.Clear();
        SetBusy(true);
        try
        {
            var password = string.Empty;
            if (item.Reprompt)
            {
                var entered = await PromptMasterPasswordAsync();
                if (entered is null || !IsCurrent(version, token)) return;
                password = entered;
            }
            var detail = await Task.Run(() => RustCore.GetVaultDetail(item.Id, password, token), token);
            password = string.Empty;
            if (IsCurrent(version, token)) ShowDetail(detail);
            else if ((detail.TotpRequestId ?? detail.AttachmentRequestId) is { } staleRequest)
                RustCore.CloseVaultDetail(staleRequest);
        }
        catch (OperationCanceledException) { }
        catch (VaultException error)
        {
            if (IsCurrent(version, token))
            {
                DetailPanel.Children.Add(new TextBlock { Text = error.UserMessage, TextWrapping = TextWrapping.Wrap });
                var retry = new Button { Content = "重试", HorizontalAlignment = HorizontalAlignment.Left };
                retry.Click += (_, _) => { VaultList.SelectedItem = null; VaultList.SelectedItem = item; };
                DetailPanel.Children.Add(retry);
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请重新构建或修复应用安装。", InfoBarSeverity.Error); }
        finally { if (version == _version) SetBusy(false); }
    }

    private void ShowDetail(VaultDetail detail)
    {
        DetailPanel.Children.Clear();
        var header = new StackPanel { Spacing = 4 };
        header.Children.Add(new TextBlock { Text = detail.Name, Style = (Style)Application.Current.Resources["SubtitleTextBlockStyle"], TextWrapping = TextWrapping.Wrap });
        header.Children.Add(new TextBlock
        {
            Text = VaultTypes.Label(detail.ItemType),
            Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"],
            Opacity = 0.72,
        });
        DetailPanel.Children.Add(header);
        if (detail.Restricted || detail.PasswordHidden) DetailPanel.Children.Add(new InfoBar { IsOpen = true, IsClosable = false, Severity = InfoBarSeverity.Warning,
            Message = detail.Restricted ? "此条目受组织访问策略限制，仅显示可用的信息。" : "组织权限禁止查看密码和隐藏字段。" });
        if (detail.Totp is { Status: "ready", Value: { } code } && detail.TotpRequestId is { } requestId)
        {
            _totpView = new TotpCodeView(code, requestId);
            DetailPanel.Children.Add(_totpView.Element);
        }
        else if (detail.Totp is { } totp)
        {
            DetailPanel.Children.Add(new InfoBar
            {
                IsOpen = true,
                IsClosable = false,
                Title = "验证码（TOTP）",
                Severity = InfoBarSeverity.Informational,
                Message = totp.Status == "premiumRequired"
                    ? "生成验证码需要个人或组织的高级会员资格。"
                    : "请在编辑条目中检查验证器密钥或 otpauth 链接。",
            });
        }
        if (detail.Passkeys is { Length: > 0 } passkeys)
        {
            var section = new StackPanel { Spacing = 6 };
            section.Children.Add(new TextBlock
            {
                Text = "通行密钥",
                Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"],
            });
            foreach (var passkey in passkeys)
            {
                section.Children.Add(new TextBlock
                {
                    Text = $"{passkey.DisplayText}\n创建于 {passkey.CreatedAt.ToLocalTime():yyyy-MM-dd HH:mm}",
                    TextWrapping = TextWrapping.Wrap,
                });
            }
            DetailPanel.Children.Add(section);
        }
        var bodyStrong = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"];
        var captionStyle = (Style)Application.Current.Resources["CaptionTextBlockStyle"];
        foreach (var field in detail.Fields)
        {
            var panel = new StackPanel { Spacing = 6 };
            var labelRow = new Grid { ColumnSpacing = 6 };
            labelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            if (field.LaunchUri is { Length: > 0 } launchUri)
                labelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            labelRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            labelRow.Children.Add(new TextBlock { Text = field.Label, Style = bodyStrong, VerticalAlignment = VerticalAlignment.Center });
            var copyColumn = 1;
            if (field.LaunchUri is { Length: > 0 } targetUri)
            {
                var launch = new Button { Content = "打开", Padding = new Thickness(10, 3, 10, 3) };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(launch, $"打开{field.Label}");
                launch.Click += async (_, _) =>
                {
                    if (Uri.TryCreate(targetUri, UriKind.Absolute, out var uri))
                        await Windows.System.Launcher.LaunchUriAsync(uri);
                };
                Grid.SetColumn(launch, 1);
                labelRow.Children.Add(launch);
                copyColumn = 2;
            }
            var copy = new Button { Content = "复制", Padding = new Thickness(10, 3, 10, 3) };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copy, $"复制{field.Label}");
            copy.Click += (_, _) =>
            {
                try { ClipboardCleaner.Copy(field.Value); copy.Content = "已复制"; }
                catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
                { copy.Content = "剪贴板暂时不可用"; }
            };
            Grid.SetColumn(copy, copyColumn);
            labelRow.Children.Add(copy);
            panel.Children.Add(labelRow);
            if (field.Sensitive) panel.Children.Add(PasswordVisibility.WithToggle(new PasswordBox { Password = field.Value }, field.Label));
            else
            {
                var value = new TextBox { Text = field.Value, IsReadOnly = true, TextWrapping = TextWrapping.Wrap, AcceptsReturn = true };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(value, field.Label);
                panel.Children.Add(value);
            }
            if (detail.ItemType == 1 && field.Sensitive && field.Label == "密码")
            {
                var metaRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
                if (detail.PasswordStrength is int score)
                {
                    metaRow.Children.Add(new TextBlock
                    {
                        Text = $"密码强度：{PasswordStrengthLabel(score)}",
                        Style = captionStyle,
                        Opacity = 0.85,
                        VerticalAlignment = VerticalAlignment.Center,
                    });
                }
                var checkButton = new Button { Content = "检查泄露", Padding = new Thickness(10, 3, 10, 3) };
                var exposedText = new TextBlock { Style = captionStyle, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
                var pwValue = field.Value;
                var detailVersion = _version;
                var detailToken = _operation?.Token ?? CancellationToken.None;
                checkButton.Click += async (_, _) => await CheckExposedAsync(pwValue, checkButton, exposedText, detailVersion, detailToken);
                metaRow.Children.Add(checkButton);
                metaRow.Children.Add(exposedText);
                panel.Children.Add(metaRow);
            }
            DetailPanel.Children.Add(panel);
        }
        ShowAttachmentDetails(detail);
        if (detail.PasswordHistory is { Length: > 0 } history)
        {
            var historyPanel = new StackPanel { Spacing = 8 };
            foreach (var entry in history)
            {
                var entryRow = new StackPanel { Spacing = 4 };
                var rowHeader = new Grid { ColumnSpacing = 8 };
                rowHeader.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                rowHeader.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                rowHeader.Children.Add(new TextBlock
                {
                    Text = $"曾使用于 {entry.LastUsedDate.ToLocalTime():yyyy-MM-dd HH:mm:ss}",
                    Style = captionStyle,
                    Opacity = 0.78,
                    VerticalAlignment = VerticalAlignment.Center,
                });
                var copyHistory = new Button { Content = "复制", Padding = new Thickness(10, 3, 10, 3) };
                var historyPw = entry.Password;
                copyHistory.Click += (_, _) =>
                {
                    try { ClipboardCleaner.Copy(historyPw); copyHistory.Content = "已复制"; }
                    catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
                    { copyHistory.Content = "剪贴板暂时不可用"; }
                };
                Grid.SetColumn(copyHistory, 1);
                rowHeader.Children.Add(copyHistory);
                entryRow.Children.Add(rowHeader);
                entryRow.Children.Add(PasswordVisibility.WithToggle(new PasswordBox { Password = entry.Password }, "历史密码"));
                historyPanel.Children.Add(entryRow);
            }
            DetailPanel.Children.Add(new Expander
            {
                Header = $"密码历史（{history.Length}）",
                Content = historyPanel,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
            });
        }
        var dates = new List<string>(3);
        if (detail.CreationDate is { } created) dates.Add($"创建时间 {created.ToLocalTime():yyyy-MM-dd HH:mm:ss}");
        dates.Add($"修改时间 {detail.RevisionDate.ToLocalTime():yyyy-MM-dd HH:mm:ss}");
        if (detail.PasswordRevisionDate is { } pwUpdated) dates.Add($"密码更新时间 {pwUpdated.ToLocalTime():yyyy-MM-dd HH:mm:ss}");
        DetailPanel.Children.Add(new TextBlock
        {
            Text = string.Join("\n", dates),
            TextWrapping = TextWrapping.Wrap,
            Style = captionStyle,
            Opacity = 0.72,
        });
    }

    internal static string PasswordStrengthLabel(int score) => score switch
    {
        0 => "极弱",
        1 => "较弱",
        2 => "一般",
        3 => "强",
        4 => "非常强",
        _ => "未知",
    };

    private async Task CheckExposedAsync(string password, Button button, TextBlock status, int version, CancellationToken token)
    {
        button.IsEnabled = false;
        status.Text = "正在检查泄露状态…";
        try
        {
            var count = await Task.Run(() => RustCore.CheckPasswordExposed(password, token), token);
            if (!IsCurrent(version, token)) return;
            status.Text = count == 0
                ? "未在已知数据泄露中发现此密码。"
                : $"警告：此密码已在 {count:N0} 次已知数据泄露中出现，建议立即更换。";
        }
        catch (OperationCanceledException) { }
        catch (VaultException error) { if (IsCurrent(version, token)) status.Text = error.UserMessage; }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) status.Text = "Rust 核心不可用。"; }
        finally { if (IsCurrent(version, token)) button.IsEnabled = true; }
    }

    private bool IsCurrent(int version, CancellationToken token) => _loaded && version == _version && !token.IsCancellationRequested;
    private void CancelOperation()
    {
        if (_attachmentRequestId is { } detailRequestId) RustCore.CloseVaultDetail(detailRequestId);
        _attachmentRequestId = null;
        _totpView?.Dispose();
        _totpView = null;
        _operation?.Cancel();
        _operation?.Dispose();
        _operation = null;
    }
    private void SetBusy(bool busy)
    {
        _busy = busy;
        LoadingRing.IsActive = busy;
        LoadingRing.Visibility = CancelRequestButton.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        FolderFilter.IsEnabled = SortChoice.IsEnabled = FolderCrumbs.IsEnabled = ChildFolderList.IsEnabled = FolderAncestorsButton.IsEnabled = !busy;
        if (busy) FolderAncestorsFlyout.Hide();
        UpdateMutationCommands();
    }
    private void SetNotice(string title, string message, InfoBarSeverity severity)
    {
        ConnectionNotice.Title = title;
        ConnectionNotice.Message = message;
        ConnectionNotice.Severity = severity;
        ConnectionNotice.IsClosable = _overview is not null && severity == InfoBarSeverity.Success;
        ConnectionNotice.IsOpen = true;
    }
    private static bool IsCoreUnavailable(Exception error) => error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;
    private void FolderFilter_SelectionChanged(object sender, SelectionChangedEventArgs e) { if (VaultList is not null) ApplyFilters(); }
    private void CancelRequest_Click(object sender, RoutedEventArgs e) => _operation?.Cancel();
    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => SignInRequested?.Invoke(this, EventArgs.Empty);
}
