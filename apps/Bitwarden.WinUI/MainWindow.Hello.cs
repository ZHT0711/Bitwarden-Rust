using Bitwarden_WinUI.Controls;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private ulong HelloWindowId => AppWindow.Id.Value;

    private Task UnlockLocalVaultAsync(string? helloAccount, string password, CancellationToken token)
    {
        var windowId = HelloWindowId;
        return Task.Run(() =>
        {
            if (helloAccount is not null) RustCore.UnlockHello(helloAccount, token, windowId);
            else RustCore.Unlock(password, token);
        });
    }

    private async Task ChangeHelloUnlockAsync(SettingsPage page)
    {
        if (_closed || _signInDialogOpen || _accountTransition || ActiveAccount is not { State: VaultSessionState.Unlocked } account) return;
        _signInDialogOpen = true;
        UpdateSessionControls(VaultSessionState.Unlocked);
        string? feedback = null;
        try
        {
            if (account.HelloEnabled)
            {
                await Task.Run(() => RustCore.DisableHello(account.Id));
                feedback = "已关闭 Windows Hello 解锁。";
            }
            else if (await ConfirmHelloEnrollmentAsync(account.Id))
            {
                feedback = "已启用 Windows Hello 解锁。下次锁定后可选择 Windows Hello。";
            }
            var accounts = await Task.Run(RustCore.GetAccounts);
            if (!_closed) ApplyAccountSnapshot(accounts);
        }
        catch (VaultException error) { feedback = error.UserMessage; }
        catch (Exception error) when (IsCoreUnavailable(error)) { feedback = "无法加载 Rust 核心，请修复应用安装。"; }
        finally
        {
            _signInDialogOpen = false;
            if (!_closed)
            {
                UpdateSessionControls(RustCore.SessionState);
                await ApplyPendingWindowsTimeoutAsync();
                if (feedback is not null && ActiveAccount?.Id == account.Id && ReferenceEquals(NavFrame.Content, page))
                    page.ReportHelloUnlock(feedback);
            }
        }
    }

    private async Task<bool> ConfirmHelloEnrollmentAsync(string account)
    {
        var password = new PasswordBox { Header = "主密码", MinWidth = 300 };
        var status = new TextBlock { Text = "先确认主密码，再按照 Windows Hello 提示验证。", TextWrapping = TextWrapping.Wrap, MaxWidth = 360 };
        var panel = new StackPanel { Spacing = 8, Children = { PasswordVisibility.WithToggle(password), status } };
        var dialog = new ContentDialog
        {
            XamlRoot = NavView.XamlRoot,
            Title = "启用 Windows Hello 解锁",
            Content = panel,
            PrimaryButtonText = "继续",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        var enrolled = false;
        password.PasswordChanged += (_, _) => dialog.IsPrimaryButtonEnabled = _unlockRequest is null && password.Password.Length != 0;
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
            using var request = _unlockRequest = new CancellationTokenSource();
            password.Password = string.Empty;
            password.IsEnabled = dialog.IsPrimaryButtonEnabled = false;
            status.Text = "正在验证，请留意 Windows Hello 提示…";
            try
            {
                var windowId = HelloWindowId;
                await Task.Run(() => RustCore.EnableHello(account, entered, request.Token, windowId));
                enrolled = true;
            }
            catch (OperationCanceledException) { status.Text = "已取消设置。"; }
            catch (VaultException error) { status.Text = error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error)) { status.Text = "无法加载 Rust 核心，请修复应用安装。"; }
            finally
            {
                entered = string.Empty;
                _unlockRequest = null;
                if (!_closed)
                {
                    if (enrolled || request.IsCancellationRequested) dialog.Hide();
                    else { password.IsEnabled = true; password.Focus(FocusState.Programmatic); }
                }
            }
        };
        try { await dialog.ShowAsync(); }
        finally { password.Password = string.Empty; }
        return enrolled;
    }
}
