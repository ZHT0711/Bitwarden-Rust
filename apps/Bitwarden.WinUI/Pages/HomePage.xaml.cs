using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Bitwarden_WinUI.Pages;

public sealed partial class HomePage : Page
{
    public event EventHandler? SignInRequested;
    private VaultOverview? _overview;
    private string _category = "vault";
    private string _query = string.Empty;
    private bool _loaded;
    private bool _syncing;
    private int _version;
    private CancellationTokenSource? _operation;

    public HomePage()
    {
        InitializeComponent();
        PopulateFolders();
        Loaded += async (_, _) => { _loaded = true; await RefreshAsync(false); };
        Unloaded += (_, _) => { _loaded = false; ClearSensitiveState(); };
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        _category = e.Parameter as string ?? "vault";
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

    public void SetSearch(string query) { _query = query.Trim(); ApplyFilters(); }
    public void ShowAuthenticated() => _ = RefreshAsync(true);

    public void ShowLoggedOut()
    {
        ClearSensitiveState();
        SetNotice("账户已退出", "当前没有活跃的账户会话。", InfoBarSeverity.Informational);
        ConnectButton.Visibility = Visibility.Visible;
        EmptyMessage.Text = "登录以查看你的保险库";
    }

    public void ClearSensitiveState()
    {
        AbortMutationFlow();
        _version++;
        CancelOperation();
        _overview = null;
        _editorOpen = false;
        _requiresSync = false;
        _syncing = false;
        VaultList.ItemsSource = null;
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock { Text = "选择一个条目查看详情" });
        PopulateFolders();
        ApplyFilters();
        SetBusy(false);
    }

    public async Task RefreshAsync(bool forceSync)
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
            if (!RustCore.IsAuthenticated) { ShowLoggedOut(); return; }
            ConnectButton.Visibility = Visibility.Collapsed;
            SetNotice("正在加载保险库", forceSync ? "正在同步并在本机解密…" : "正在读取本次会话的保险库…", InfoBarSeverity.Informational);
            VaultOverview result;
            try
            {
                result = forceSync ? await Task.Run(() => RustCore.SyncVault(token), token)
                    : await Task.Run(RustCore.GetVaultOverview, token);
            }
            catch (VaultException error) when (!forceSync && (error.Code is "notFound" or "notSynced"))
            {
                result = await Task.Run(() => RustCore.SyncVault(token), token);
            }
            if (!IsCurrent(version, token)) return;
            _overview = result;
            _requiresSync = false;
            PopulateFolders();
            ApplyFilters();
            SetNotice(result.Failures > 0 ? "保险库已加载，部分条目无法读取" : "保险库已加载",
                $"共 {result.Items.Length} 个条目；上次同步 {result.LastSync.ToLocalTime():MM-dd HH:mm:ss}。" +
                (result.Failures > 0 ? $" {result.Failures} 个条目或文件夹解析 / 解密失败。" : string.Empty),
                result.Failures > 0 ? InfoBarSeverity.Warning : InfoBarSeverity.Success);
        }
        catch (OperationCanceledException)
        {
            if (_loaded && version == _version) SetNotice("请求已取消", "你可以重新同步。", InfoBarSeverity.Informational);
        }
        catch (VaultException error) { if (IsCurrent(version, token)) SetNotice("无法加载保险库", error.UserMessage, InfoBarSeverity.Error); }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请重新构建或修复应用安装。", InfoBarSeverity.Error); }
        finally
        {
            if (version == _version) { _syncing = false; SetBusy(false); ApplyFilters(); }
        }
    }

    private void PopulateFolders()
    {
        var selected = (FolderFilter.SelectedItem as ComboBoxItem)?.Tag as string;
        FolderFilter.Items.Clear();
        FolderFilter.Items.Add(new ComboBoxItem { Content = "所有文件夹", Tag = "*" });
        FolderFilter.Items.Add(new ComboBoxItem { Content = "无文件夹", Tag = "" });
        foreach (var folder in _overview?.Folders ?? []) FolderFilter.Items.Add(new ComboBoxItem { Content = folder.Name, Tag = folder.Id });
        FolderFilter.SelectedItem = FolderFilter.Items.OfType<ComboBoxItem>().FirstOrDefault(item => Equals(item.Tag, selected)) ?? FolderFilter.Items[0];
    }

    private void ApplyFilters()
    {
        var folder = (FolderFilter.SelectedItem as ComboBoxItem)?.Tag as string ?? "*";
        var items = VaultQuery.Apply(_overview?.Items ?? [], _category, folder, _query);
        VaultList.ItemsSource = items;
        ItemCount.Text = $"{items.Length} 个条目";
        EmptyState.Visibility = items.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        EmptyMessage.Text = _overview is null ? "登录或同步以查看你的保险库" : _query.Length > 0 ? "没有匹配的条目" : "此分类或文件夹中没有条目";
        UpdateMutationCommands();
    }

    private async void VaultList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_syncing || _editorOpen) return;
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
                var passwordBox = new PasswordBox { Header = "主密码" };
                var dialog = new ContentDialog { Title = "验证主密码", Content = passwordBox, PrimaryButtonText = "验证",
                    CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary, XamlRoot = XamlRoot };
                var answer = await dialog.ShowAsync();
                password = passwordBox.Password;
                passwordBox.Password = string.Empty;
                if (answer != ContentDialogResult.Primary || !IsCurrent(version, token)) return;
            }
            var detail = await Task.Run(() => RustCore.GetVaultDetail(item.Id, password, token), token);
            password = string.Empty;
            if (IsCurrent(version, token)) ShowDetail(detail);
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
        foreach (var field in detail.Fields)
        {
            var panel = new StackPanel { Spacing = 6 };
            panel.Children.Add(new TextBlock { Text = field.Label, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
            if (field.Sensitive) panel.Children.Add(new PasswordBox { Password = field.Value, PasswordRevealMode = PasswordRevealMode.Peek });
            else panel.Children.Add(new TextBox { Text = field.Value, IsReadOnly = true, TextWrapping = TextWrapping.Wrap, AcceptsReturn = true });
            var copy = new Button { Content = "复制", HorizontalAlignment = HorizontalAlignment.Left };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copy, $"复制{field.Label}");
            copy.Click += (_, _) =>
            {
                try { ClipboardCleaner.Copy(field.Value); copy.Content = "已复制，30 秒后清除"; }
                catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
                { copy.Content = "剪贴板暂时不可用"; }
            };
            panel.Children.Add(copy);
            DetailPanel.Children.Add(panel);
        }
        if (detail.Attachments > 0) DetailPanel.Children.Add(new TextBlock { Text = $"{detail.Attachments} 个附件（下载功能待接入）", TextWrapping = TextWrapping.Wrap });
        DetailPanel.Children.Add(new TextBlock { Text = $"修改时间 {detail.RevisionDate.ToLocalTime():yyyy-MM-dd HH:mm:ss}", TextWrapping = TextWrapping.Wrap });
    }

    private bool IsCurrent(int version, CancellationToken token) => _loaded && version == _version && !token.IsCancellationRequested;
    private void CancelOperation() { _operation?.Cancel(); _operation?.Dispose(); _operation = null; }
    private void SetBusy(bool busy)
    {
        _busy = busy;
        LoadingRing.IsActive = busy;
        LoadingRing.Visibility = CancelRequestButton.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        FolderFilter.IsEnabled = !busy;
        UpdateMutationCommands();
    }
    private void SetNotice(string title, string message, InfoBarSeverity severity)
    { ConnectionNotice.Title = title; ConnectionNotice.Message = message; ConnectionNotice.Severity = severity; ConnectionNotice.IsOpen = true; }
    private static bool IsCoreUnavailable(Exception error) => error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;
    private void FolderFilter_SelectionChanged(object sender, SelectionChangedEventArgs e) { if (VaultList is not null) ApplyFilters(); }
    private void CancelRequest_Click(object sender, RoutedEventArgs e) => _operation?.Cancel();
    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => SignInRequested?.Invoke(this, EventArgs.Empty);
}
