using Bitwarden_WinUI.Controls;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private DispatcherTimer? _passkeyTimer;
    private bool _passkeyChecking;
    private ContentDialog? _passkeyDialog;
    private Action? _clearPasskeyApproval;

    private void ClearPasskeySensitiveState()
    {
        try { RustCore.CancelPasskey(string.Empty); }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
        if (_clearPasskeyApproval is { } clear) clear();
        else _passkeyDialog?.Hide();
    }

    private async void StartPasskeyProvider()
    {
        try { await Task.Run(RustCore.StartPasskeys); }
        catch (VaultException) { return; }
        catch (Exception error) when (IsCoreUnavailable(error)) { return; }
        if (_closed) return;
        _passkeyTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(350) };
        _passkeyTimer.Tick += CheckPasskeyRequest;
        _passkeyTimer.Start();
    }

    private async void CheckPasskeyRequest(object? sender, object e)
    {
        if (_closed || _passkeyChecking || _signInDialogOpen || _accountTransition) return;
        _passkeyChecking = true;
        try
        {
            var request = await Task.Run(RustCore.PendingPasskey);
            if (_closed || _accountTransition || string.IsNullOrEmpty(request.Id)) return;
            var requestId = request.Id;
            Activate();
            if (request.SessionState == VaultSessionState.SignedOut) await ShowSignInDialogAsync();
            else if (request.SessionState == VaultSessionState.Locked) await ShowUnlockDialogAsync();
            if (_closed) return;
            if (RustCore.SessionState != VaultSessionState.Unlocked) { RustCore.CancelPasskey(requestId); return; }
            request = await Task.Run(RustCore.PendingPasskey);
            if (!request.IsFor(requestId)) return;
            var ready = request.NeedsSync ? await SynchronizePasskeyVaultAsync(request) : request;
            if (!_closed && !_accountTransition && ready is not null && ready.IsReadyFor(requestId) &&
                RustCore.SessionState == VaultSessionState.Unlocked && RustCore.IsPasskeyActive(requestId))
                await ShowPasskeyDialogAsync(ready);
        }
        catch (VaultException) { }
        catch (Exception error) when (IsCoreUnavailable(error) || error is System.Runtime.InteropServices.COMException)
        {
            // A vault editor or sign-in dialog may already occupy this XamlRoot. Poll again later.
        }
        finally { _passkeyChecking = false; }
    }

    private async Task<PasskeyPending?> SynchronizePasskeyVaultAsync(PasskeyPending request)
    {
        using var cancellation = new CancellationTokenSource();
        var cancellationToken = cancellation.Token;
        var progress = new ProgressRing { IsActive = true, Width = 24, Height = 24,
            HorizontalAlignment = HorizontalAlignment.Left };
        var status = new TextBlock { Text = "正在同步保险库…", TextWrapping = TextWrapping.Wrap, MaxWidth = 480 };
        var panel = new StackPanel { Spacing = 12, MaxWidth = 480 };
        panel.Children.Add(new TextBlock { Text = request.RelyingParty ?? "", FontSize = 20, TextWrapping = TextWrapping.Wrap });
        panel.Children.Add(progress);
        panel.Children.Add(status);
        var dialog = new ContentDialog {
            XamlRoot = NavView.XamlRoot, Title = "准备通行密钥保险库", Content = panel,
            PrimaryButtonText = "重试同步", CloseButtonText = "取消", IsPrimaryButtonEnabled = false,
            DefaultButton = ContentDialogButton.Close,
        };
        AutomationProperties.SetAutomationId(dialog, "PasskeySyncDialog");
        bool busy = false, opened = false, closed = false;
        PasskeyPending? ready = null;
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(350) };
        timer.Tick += (_, _) => { if (!RustCore.IsPasskeyActive(request.Id)) dialog.Hide(); };

        async Task SynchronizeAsync()
        {
            if (busy || closed) return;
            busy = true;
            progress.IsActive = true;
            dialog.IsPrimaryButtonEnabled = false;
            status.Text = "正在同步保险库…";
            try {
                await Task.Run(() => RustCore.SyncVault(cancellationToken));
                cancellationToken.ThrowIfCancellationRequested();
                if (closed || _closed) return;
                var refreshed = await Task.Run(RustCore.PendingPasskey);
                if (closed || _closed) return;
                if (!refreshed.IsFor(request.Id)) { dialog.Hide(); return; }
                if (!refreshed.IsReadyFor(request.Id)) throw new VaultException("notSynced");
                ready = refreshed;
                dialog.Hide();
            }
            catch (OperationCanceledException) { if (!closed) dialog.Hide(); }
            catch (VaultException error) { if (!closed) status.Text = error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error))
            { if (!closed) status.Text = "无法加载 Rust 核心。"; }
            finally {
                busy = false;
                if (!closed) {
                    progress.IsActive = false;
                    dialog.IsPrimaryButtonEnabled = RustCore.IsPasskeyActive(request.Id) &&
                        RustCore.SessionState == VaultSessionState.Unlocked;
                }
            }
        }

        dialog.Opened += async (_, _) => { opened = true; timer.Start(); await SynchronizeAsync(); };
        dialog.PrimaryButtonClick += async (_, args) => { args.Cancel = true; await SynchronizeAsync(); };
        dialog.Closing += (_, _) => {
            closed = true;
            cancellation.Cancel();
            if (ready is null) RustCore.CancelPasskey(request.Id);
        };
        try { _passkeyDialog = dialog; await dialog.ShowAsync(); }
        finally {
            _passkeyDialog = null;
            closed = true;
            timer.Stop();
            cancellation.Cancel();
            if (opened && ready is null) RustCore.CancelPasskey(request.Id);
        }
        return ready;
    }

    private async Task ShowPasskeyDialogAsync(PasskeyPending request)
    {
        var choices = new ComboBox { Header = request.Creating ? "保存到" : "选择账户", MinWidth = 320, MaxWidth = 480,
            HorizontalAlignment = HorizontalAlignment.Stretch };
        AutomationProperties.SetAutomationId(choices, "PasskeyAccountChoice");
        if (request.Creating) choices.Items.Add("新建登录条目");
        foreach (var choice in request.Choices ?? []) choices.Items.Add(choice);
        if (choices.Items.Count > 0) choices.SelectedIndex = 0;
        var password = new PasswordBox { Header = "此条目需要重新验证主密码" };
        var passwordControl = PasswordVisibility.WithToggle(password);
        passwordControl.Visibility = Visibility.Collapsed;
        choices.SelectionChanged += (_, _) => passwordControl.Visibility = choices.SelectedItem is PasskeyChoice { Reprompt: true }
            ? Visibility.Visible : Visibility.Collapsed;
        choices.SelectedIndex = -1;
        if (choices.Items.Count > 0) choices.SelectedIndex = 0;
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap, MaxWidth = 480 };
        var panel = new StackPanel { Spacing = 12, MaxWidth = 480 };
        panel.Children.Add(new TextBlock { Text = request.RelyingParty ?? "", FontSize = 20, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true });
        if (!string.IsNullOrWhiteSpace(request.UserName)) panel.Children.Add(new TextBlock { Text = request.UserName, TextWrapping = TextWrapping.Wrap });
        panel.Children.Add(new TextBlock { Text = "Windows Hello 将验证本次通行密钥请求。", TextWrapping = TextWrapping.Wrap });
        if (choices.Items.Count == 0) panel.Children.Add(new TextBlock { Text = "保险库中没有此网站可用的通行密钥。", TextWrapping = TextWrapping.Wrap });
        else panel.Children.Add(choices);
        panel.Children.Add(passwordControl);
        panel.Children.Add(status);
        var dialog = new ContentDialog {
            XamlRoot = NavView.XamlRoot, Title = request.Creating ? "保存通行密钥" : "使用通行密钥登录",
            Content = panel, PrimaryButtonText = request.Creating ? "验证并保存" : "验证并登录", CloseButtonText = "取消",
            IsPrimaryButtonEnabled = choices.Items.Count > 0, DefaultButton = ContentDialogButton.Close,
        };
        AutomationProperties.SetAutomationId(dialog, "PasskeyApprovalDialog");
        bool busy = false, completed = false, opened = false, failed = false, closeRequested = false, aborted = false;
        void UpdateApprovalButton()
        {
            if (!aborted && !busy && !failed) dialog.IsPrimaryButtonEnabled = choices.SelectedIndex >= 0 &&
                (choices.SelectedItem is not PasskeyChoice { Reprompt: true } || password.Password.Length > 0);
        }
        choices.SelectionChanged += (_, _) => UpdateApprovalButton();
        password.PasswordChanged += (_, _) => UpdateApprovalButton();
        UpdateApprovalButton();
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(350) };
        timer.Tick += (_, _) => { if (!busy && !failed && !RustCore.IsPasskeyActive(request.Id)) dialog.Hide(); };
        dialog.Opened += (_, _) => { opened = true; timer.Start(); };
        dialog.Closing += (_, args) => {
            if (!completed) RustCore.CancelPasskey(request.Id);
            if (busy && !aborted) { closeRequested = true; args.Cancel = true; }
        };
        dialog.PrimaryButtonClick += async (_, args) => {
            args.Cancel = true;
            if (aborted || busy || !RustCore.IsPasskeyActive(request.Id)) return;
            busy = true;
            dialog.IsPrimaryButtonEnabled = choices.IsEnabled = password.IsEnabled = false;
            var choice = choices.SelectedItem as PasskeyChoice;
            var entered = password.Password;
            password.Password = string.Empty;
            status.Text = "正在验证并处理请求…";
            var window = WinRT.Interop.WindowNative.GetWindowHandle(this);
            try {
                await Task.Run(() => RustCore.RespondPasskey(request.Id, choice, entered, window));
                completed = true;
            }
            catch (OperationCanceledException) { closeRequested = true; }
            catch (VaultException error) { failed = true; if (!aborted) status.Text = error.Code == "permissionDenied"
                ? "本次 Windows Hello 验证未完成，或条目权限不足。请关闭后重新发起请求。" : error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error)) { failed = true; if (!aborted) status.Text = "无法加载 Rust 核心。"; }
            finally { entered = string.Empty; busy = false; }
            if (_closed || aborted) return;
            if (completed || closeRequested) dialog.Hide();
            else if (failed) dialog.CloseButtonText = "关闭";
        };
        try {
            _passkeyDialog = dialog;
            _clearPasskeyApproval = () => {
                aborted = true;
                password.Password = string.Empty;
                choices.Items.Clear();
                panel.Children.Clear();
                dialog.Hide();
            };
            await dialog.ShowAsync();
        }
        finally {
            _clearPasskeyApproval = null;
            _passkeyDialog = null;
            timer.Stop();
            password.Password = string.Empty;
            if (opened && !completed) RustCore.CancelPasskey(request.Id);
        }
        if (completed && !aborted && !_closed && !_accountTransition)
        {
            if (NavFrame.Content is HomePage page) await page.RefreshAsync(false);
            else if (NavFrame.Content is FolderManagementPage folders) await folders.RefreshAsync(false);
        }
    }
}
