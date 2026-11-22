using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

public sealed partial class SendsPage : Page
{
    internal event EventHandler? ConnectRequested;
    private VaultAccount? _account;
    private SendSummary[]? _items;
    private string _search = string.Empty;
    private CancellationTokenSource? _work;
    private ContentDialog? _dialog;
    private Action? _clearDialog;
    private bool _loaded;
    private bool _requiresRefresh;
    private bool _online => _account is { State: VaultSessionState.Unlocked, NeedsSignIn: false };

    public SendsPage()
    {
        InitializeComponent();
        Loaded += async (_, _) => { _loaded = true; await RefreshAsync(); };
        Unloaded += (_, _) => { _loaded = false; ClearSensitiveState(); };
    }

    internal async Task ShowAccountAsync(VaultAccount? account, string search)
    {
        if (account != _account) ClearSensitiveState();
        _account = account;
        SetSearch(search);
        await RefreshAsync();
    }

    internal void SetSearch(string search)
    {
        _search = search.Trim();
        RenderRows();
    }

    internal void ClearSensitiveState()
    {
        _account = null;
        _work?.Cancel();
        _work = null;
        _clearDialog?.Invoke();
        _dialog?.Hide();
        _dialog = null;
        _clearDialog = null;
        _items = null;
        _search = string.Empty;
        _requiresRefresh = false;
        SendNotice.IsOpen = false;
        RenderRows();
    }

    internal async Task RefreshAsync()
    {
        if (!_loaded || _dialog is not null || _work is not null) return;
        if (!_online)
        {
            _items = null;
            RenderRows();
            return;
        }
        await RunAsync(RustCore.ListSends, overview =>
        {
            _items = overview.Items;
            _requiresRefresh = false;
            SendNotice.IsOpen = false;
            if (overview.Failures != 0)
                Notice("部分 Send 无法读取", $"有 {overview.Failures} 项无法解密或格式无效。", InfoBarSeverity.Warning);
            RenderRows();
        });
    }

    private async Task RunAsync<T>(Func<string, CancellationToken, T> operation, Action<T> complete, Action<string>? report = null, bool acknowledgedWrite = false)
    {
        if (!_loaded || !_online || _account is not { } account || _work is not null) return;
        using var work = _work = new CancellationTokenSource();
        var token = work.Token;
        UpdateCommands();
        try
        {
            var result = await Task.Run(() => operation(account.Id, token), token);
            if (ReferenceEquals(_work, work) && (acknowledgedWrite || !token.IsCancellationRequested)) complete(result);
        }
        catch (OperationCanceledException) { }
        catch (VaultException error)
        {
            if (!ReferenceEquals(_work, work)) return;
            if (error.Code is "writeUncertain" or "conflict") _requiresRefresh = true;
            else if (token.IsCancellationRequested) return;
            var message = error.Code switch
            {
                "validation" => "服务器未接受 Send，请检查内容、日期和访问选项。",
                "unsupported" => "当前服务器或此 Send 类型不支持该操作。",
                "attachmentTooLarge" => "所选文件超过服务器或桌面端的大小限制，或可用内存不足。",
                "attachmentFileRead" => "无法读取所选文件。请检查文件是否仍然存在且可访问。",
                "sendFileUploadFailed" => "文件上传未完成，已在服务器上删除本次创建的 Send。",
                "network" => "无法连接 Send 服务，请检查网络后重试。",
                "writeUncertain" => "服务器可能已保存此操作，不能直接重试。请关闭对话框并刷新 Send 列表核对。",
                "conflict" => "此 Send 已发生变化。请关闭对话框、刷新列表并核对后再修改。",
                _ => error.UserMessage,
            };
            if (report is not null) report(message);
            else Notice("Send 操作未完成", message, InfoBarSeverity.Error);
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            if (!ReferenceEquals(_work, work)) return;
            if (report is not null) report("无法加载 Rust 核心。");
            else Notice("无法加载 Rust 核心", "请修复应用安装。", InfoBarSeverity.Error);
        }
        finally
        {
            if (ReferenceEquals(_work, work))
            {
                _work = null;
                UpdateCommands();
            }
        }
    }

    private void RenderRows()
    {
        var selected = (SendList.SelectedItem as SendSummary)?.Id;
        var rows = _items is null ? [] : _search.Length == 0 ? _items :
            _items.Where(item => item.Name.Contains(_search, StringComparison.OrdinalIgnoreCase)).ToArray();
        SendList.ItemsSource = rows;
        if (selected is not null) SendList.SelectedItem = rows.FirstOrDefault(item => item.Id == selected);
        SendCount.Text = $"{rows.Length} 个 Send";
        EmptyState.Visibility = rows.Length != 0 || _work is not null ? Visibility.Collapsed : Visibility.Visible;
        ConnectButton.Visibility = _online ? Visibility.Collapsed : Visibility.Visible;
        ConnectButton.Content = _account?.State switch
        {
            VaultSessionState.Locked => "解锁保险库",
            VaultSessionState.Unlocked => "重新连接账户",
            _ => "登录",
        };
        EmptyMessage.Text = !_online ? "Send 需要已解锁并在线的账户。" : _items is null ? "刷新以加载 Send。" :
            _search.Length == 0 ? "还没有 Send。" : "没有匹配的 Send。";
        UpdateCommands();
    }

    private void UpdateCommands()
    {
        LoadingRing.IsActive = _work is not null;
        var available = _online && _loaded && _work is null && _dialog is null;
        var selected = SendList.SelectedItem is SendSummary;
        SendList.IsEnabled = available;
        NewSendButton.IsEnabled = available && _items is not null && !_requiresRefresh;
        OpenSendButton.IsEnabled = CopySendButton.IsEnabled = available && selected;
        DeleteSendButton.IsEnabled = available && selected && !_requiresRefresh;
    }

    private void Notice(string title, string message, InfoBarSeverity severity)
    {
        SendNotice.Title = title;
        SendNotice.Message = message;
        SendNotice.Severity = severity;
        SendNotice.IsOpen = true;
    }

    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => ConnectRequested?.Invoke(this, EventArgs.Empty);
    private void SendList_SelectionChanged(object sender, SelectionChangedEventArgs e) => UpdateCommands();
    private async void SendList_ItemClick(object sender, ItemClickEventArgs e) { if (e.ClickedItem is SendSummary item) await OpenAsync(item); }
    private async void OpenSend_Click(object sender, RoutedEventArgs e) { if (SendList.SelectedItem is SendSummary item) await OpenAsync(item); }
    private async void NewTextSend_Click(object sender, RoutedEventArgs e) { if (NewSendButton.IsEnabled) await ShowEditorAsync(null); }
    private async void NewFileSend_Click(object sender, RoutedEventArgs e) { if (NewSendButton.IsEnabled) await ShowEditorAsync(null, newFile: true); }

    private async Task OpenAsync(SendSummary item)
    {
        if (_dialog is not null || _work is not null) return;
        SendDetail? detail = null;
        var account = _account;
        await RunAsync((id, token) => RustCore.GetSend(id, item.Id, token), value => detail = value);
        if (detail is not null && _loaded && account == _account && _online)
            await ShowDetailAsync(detail);
    }

    private async void CopySend_Click(object sender, RoutedEventArgs e)
    {
        if (SendList.SelectedItem is not SendSummary item || _dialog is not null) return;
        await RunAsync((id, token) => RustCore.GetSendLink(id, item.Id, token), link =>
        {
            try { ClipboardCleaner.Copy(link); Notice("已复制 Send 链接", "持有链接的人可能可访问内容，请只发给预期接收者。", InfoBarSeverity.Success); }
            catch (Exception error) when (error is InvalidOperationException or System.Runtime.InteropServices.COMException)
            { Notice("未能复制链接", "剪贴板暂不可用。", InfoBarSeverity.Error); }
        });
    }

    private async void DeleteSend_Click(object sender, RoutedEventArgs e)
    {
        if (!DeleteSendButton.IsEnabled || SendList.SelectedItem is not SendSummary item) return;
        var account = _account;
        var dialog = new ContentDialog { XamlRoot = XamlRoot, Title = "删除 Send？",
            Content = new TextBlock { Text = $"将永久删除「{item.Name}」，共享链接也会失效。", TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = "永久删除", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
        if (await ShowDialogAsync(dialog, () => dialog.Content = null) != ContentDialogResult.Primary || account != _account) return;
        var deleted = false;
        await RunAsync((id, token) => { RustCore.DeleteSend(id, item, token); return true; }, ok => deleted = ok);
        if (deleted) await RefreshAsync();
    }

    private async Task<ContentDialogResult> ShowDialogAsync(ContentDialog dialog, Action clear)
    {
        if (_dialog is not null || !_loaded || !_online) return ContentDialogResult.None;
        _dialog = dialog;
        _clearDialog = clear;
        UpdateCommands();
        try { return await dialog.ShowAsync(); }
        catch (System.Runtime.InteropServices.COMException) { return ContentDialogResult.None; }
        finally
        {
            clear();
            if (ReferenceEquals(_dialog, dialog)) { _dialog = null; _clearDialog = null; }
            UpdateCommands();
        }
    }
}
