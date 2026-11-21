using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Bitwarden_WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private bool _accountTransition;
    private bool _closed;
    private CancellationTokenSource? _unlockRequest;

    private void UpdateSessionControls(VaultSessionState state)
    {
        var unlocked = state == VaultSessionState.Unlocked;
        var signedIn = state != VaultSessionState.SignedOut;
        SearchBox.IsEnabled = SyncButton.IsEnabled = unlocked && !_accountTransition;
        LockButton.IsEnabled = SignInButton.IsEnabled = !_accountTransition && !_signInDialogOpen;
        LockButton.Visibility = signedIn ? Visibility.Visible : Visibility.Collapsed;
        LockButton.Content = unlocked ? "锁定" : "解锁";
        SignInButton.Content = signedIn ? "退出登录" : "登录";
        UpdateVaultTimeoutMonitor(state);
    }

    private async void Lock_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            if (RustCore.SessionState == VaultSessionState.Locked)
            {
                await ShowUnlockDialogAsync();
            }
            else if (RustCore.SessionState == VaultSessionState.Unlocked)
            {
                await ChangeSessionAsync(LockVault);
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }

    private static void LockVault()
    {
        if (!RustCore.Lock()) RustCore.Logout();
    }

    private async Task ChangeSessionAsync(Action change)
    {
        if (_accountTransition || _signInDialogOpen) return;
        _accountTransition = true;
        DiscardPendingWindowsTimeout();
        UpdateSessionControls(RustCore.SessionState);
        ClearPasskeySensitiveState();
        NavView.IsEnabled = false;
        SearchBox.Text = string.Empty;
        if (NavFrame.Content is HomePage vault) vault.ClearSensitiveState();
        if (NavFrame.Content is FolderManagementPage folders) folders.ClearSensitiveState();
        if (NavFrame.Content is GeneratorPage generator) generator.ClearSensitiveState();
        WebsiteIcons.ClearCache();
        ClipboardCleaner.ClearOwned();

        try { await Task.Run(change); }
        finally
        {
            _accountTransition = false;
            if (!_closed)
            {
                NavView.IsEnabled = true;
                var state = RustCore.SessionState;
                UpdateSessionControls(state);
                var timeoutHandled = await ApplyPendingWindowsTimeoutAsync();
                if (!timeoutHandled && NavFrame.Content is HomePage page)
                {
                    if (state == VaultSessionState.Locked) page.ShowLocked();
                    else if (state == VaultSessionState.SignedOut) page.ShowLoggedOut();
                    else await page.RefreshAsync(false);
                }
                else if (!timeoutHandled && NavFrame.Content is FolderManagementPage folderPage)
                {
                    await folderPage.RefreshAsync(false);
                }
            }
        }
    }

    private async Task ShowUnlockDialogAsync()
    {
        if (_signInDialogOpen || _accountTransition) return;
        _signInDialogOpen = true;
        UpdateSessionControls(VaultSessionState.Locked);

        var password = new PasswordBox { Header = "主密码", MinWidth = 300 };
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap, MaxWidth = 360 };
        var panel = new StackPanel { Spacing = 8 };
        panel.Children.Add(PasswordVisibility.WithToggle(password));
        panel.Children.Add(status);
        var dialog = new ContentDialog
        {
            XamlRoot = NavView.XamlRoot,
            Title = "解锁保险库",
            Content = panel,
            PrimaryButtonText = "解锁",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };

        password.PasswordChanged += (_, _) =>
            dialog.IsPrimaryButtonEnabled = _unlockRequest is null && password.Password.Length != 0;
        dialog.Opened += (_, _) => password.Focus(FocusState.Programmatic);
        dialog.Closing += (_, args) =>
        {
            if (_unlockRequest is null) return;
            args.Cancel = true;
            _unlockRequest.Cancel();
        };
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (_unlockRequest is not null || password.Password.Length == 0) return;
            var entered = password.Password;
            DiscardPendingWindowsTimeout();
            using var request = _unlockRequest = new CancellationTokenSource();
            password.Password = string.Empty;
            password.IsEnabled = dialog.IsPrimaryButtonEnabled = false;
            dialog.CloseButtonText = "取消解锁";
            status.Text = "正在本机解锁…";
            var unlocked = false;
            try
            {
                await Task.Run(() => RustCore.Unlock(entered, request.Token));
                unlocked = true;
            }
            catch (OperationCanceledException) { status.Text = "解锁已取消。"; }
            catch (VaultException error) { status.Text = error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error))
            { status.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally
            {
                entered = string.Empty;
                _unlockRequest = null;
                if (!_closed)
                {
                    if (unlocked || request.IsCancellationRequested) dialog.Hide();
                    else
                    {
                        password.IsEnabled = true;
                        dialog.CloseButtonText = "取消";
                        password.Focus(FocusState.Programmatic);
                    }
                }
            }
        };

        try { await dialog.ShowAsync(); }
        finally
        {
            password.Password = string.Empty;
            _signInDialogOpen = false;
            if (!_closed)
            {
                var state = RustCore.SessionState;
                UpdateSessionControls(state);
                var timeoutHandled = await ApplyPendingWindowsTimeoutAsync();
                if (!timeoutHandled && state == VaultSessionState.Unlocked)
                {
                    if (NavFrame.Content is HomePage page) await page.RefreshAsync(false);
                    else if (NavFrame.Content is FolderManagementPage folderPage) await folderPage.RefreshAsync(false);
                }
            }
        }
    }
}
