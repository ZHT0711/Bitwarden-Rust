using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Bitwarden_WinUI.Pages;

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
        base.OnNavigatedTo(e);
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
        _initialFolderId = null;
        _editorOpen = false;
        _selectingItems = false;
        _requiresSync = false;
        _syncing = false;
        VaultList.ItemsSource = null;
        VaultList.SelectionMode = ListViewSelectionMode.Single;
        foreach (var field in DetailPanel.Children.OfType<StackPanel>())
        {
            foreach (var box in field.Children.OfType<Grid>().SelectMany(row => row.Children.OfType<PasswordBox>()))
                box.Password = string.Empty;
            foreach (var box in field.Children.OfType<TextBox>()) box.Text = string.Empty;
        }
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock { Text = "选择一个条目查看详情" });
        PopulateFolders();
        ApplyFilters();
        SetBusy(false);
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
        FolderChoice[] choices = [new("*", "所有文件夹"), new("", "无文件夹"),
            .. _folderHierarchy.All.Select(folder => new FolderChoice(folder.Id, folder.Name))];
        _folderChoices = choices.ToDictionary(choice => choice.Id, StringComparer.Ordinal);
        FolderFilter.ItemsSource = choices;
        FolderFilter.SelectedItem = selected is not null && _folderChoices.TryGetValue(selected, out var choice)
            ? choice : choices[0];
    }

    private void ApplyFilters()
    {
        var folder = (FolderFilter.SelectedItem as FolderChoice)?.Id ?? "*";
        var items = VaultQuery.Apply(OrderedItems(), _category, folder, _query);
        UpdateFolderNavigation(folder);
        UpdateListItems(items);
        ItemCount.Text = $"{items.Length} 个条目";
        EmptyState.Visibility = items.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        EmptyMessage.Text = _overview is null ? "登录或同步以查看你的保险库" : _query.Length > 0 ? "没有匹配的条目" : "此分类或文件夹中没有条目";
        UpdateMutationCommands();
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
        DetailPanel.Children.Add(new TextBlock { Text = detail.Name, Style = (Style)Application.Current.Resources["SubtitleTextBlockStyle"], TextWrapping = TextWrapping.Wrap });
        DetailPanel.Children.Add(new TextBlock { Text = VaultTypes.Label(detail.ItemType) });
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
        foreach (var field in detail.Fields)
        {
            var panel = new StackPanel { Spacing = 6 };
            panel.Children.Add(new TextBlock { Text = field.Label, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
            if (field.Sensitive) panel.Children.Add(PasswordVisibility.WithToggle(new PasswordBox { Password = field.Value }, field.Label));
            else
            {
                var value = new TextBox { Text = field.Value, IsReadOnly = true, TextWrapping = TextWrapping.Wrap, AcceptsReturn = true };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(value, field.Label);
                panel.Children.Add(value);
            }
            var copy = new Button { Content = "复制", HorizontalAlignment = HorizontalAlignment.Left };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copy, $"复制{field.Label}");
            copy.Click += (_, _) =>
            {
                try { ClipboardCleaner.Copy(field.Value); copy.Content = "已复制"; }
                catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
                { copy.Content = "剪贴板暂时不可用"; }
            };
            panel.Children.Add(copy);
            DetailPanel.Children.Add(panel);
        }
        ShowAttachmentDetails(detail);
        DetailPanel.Children.Add(new TextBlock { Text = $"修改时间 {detail.RevisionDate.ToLocalTime():yyyy-MM-dd HH:mm:ss}", TextWrapping = TextWrapping.Wrap });
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
    { ConnectionNotice.Title = title; ConnectionNotice.Message = message; ConnectionNotice.Severity = severity; ConnectionNotice.IsOpen = true; }
    private static bool IsCoreUnavailable(Exception error) => error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;
    private void FolderFilter_SelectionChanged(object sender, SelectionChangedEventArgs e) { if (VaultList is not null) ApplyFilters(); }
    private void CancelRequest_Click(object sender, RoutedEventArgs e) => _operation?.Cancel();
    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => SignInRequested?.Invoke(this, EventArgs.Empty);
}
