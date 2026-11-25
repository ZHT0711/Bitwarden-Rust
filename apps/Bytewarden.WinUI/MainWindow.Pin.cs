using Bytewarden.WinUI.Controls;
using Bytewarden.WinUI.Interop;
using Bytewarden.WinUI.Pages;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bytewarden.WinUI;

public sealed partial class MainWindow
{
    private async Task ChangePinUnlockAsync(SettingsPage page)
    {
        if (_closed || _signInDialogOpen || _accountTransition || ActiveAccount is not { State: VaultSessionState.Unlocked } account) return;
        _signInDialogOpen = true;
        UpdateSessionControls(VaultSessionState.Unlocked);
        string? feedback = null;
        try
        {
            if (account.PinEnabled)
            {
                await Task.Run(() => RustCore.DisablePin(account.Id));
                feedback = "已关闭 PIN 解锁。";
            }
            else if (await ConfirmPinEnrollmentAsync(account.Id))
            {
                feedback = "已启用 PIN 解锁。锁定后可使用 PIN 码快速解锁。";
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
                    page.ReportPinUnlock(feedback);
            }
        }
    }

    private async Task<bool> ConfirmPinEnrollmentAsync(string account)
    {
        var masterPassword = new PasswordBox { Header = "主密码", MinWidth = 300 };
        var pin = new PasswordBox { Header = "新 PIN 码（至少 4 个字符）", MinWidth = 300 };
        var confirmPin = new PasswordBox { Header = "确认 PIN 码", MinWidth = 300 };
        var requireMasterOnRestart = new CheckBox
        {
            Content = "应用重启后仍需要主密码解锁",
            IsChecked = true,
        };
        var status = new TextBlock
        {
            Text = "先验证当前主密码，再设置本地解锁 PIN 码。输错 5 次将自动停用 PIN。",
            TextWrapping = TextWrapping.Wrap,
            MaxWidth = 360,
        };
        var panel = new StackPanel
        {
            Spacing = 8,
            Children =
            {
                PasswordVisibility.WithToggle(masterPassword),
                PasswordVisibility.WithToggle(pin),
                PasswordVisibility.WithToggle(confirmPin),
                requireMasterOnRestart,
                status,
            },
        };
        var dialog = new ContentDialog
        {
            XamlRoot = NavView.XamlRoot,
            Title = "启用 PIN 解锁",
            Content = panel,
            PrimaryButtonText = "启用",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        var enrolled = false;
        void UpdateValidity()
        {
            if (_unlockRequest is not null)
            {
                dialog.IsPrimaryButtonEnabled = false;
                return;
            }
            var pinText = pin.Password;
            var confirmText = confirmPin.Password;
            if (pinText.Length > 0 && pinText.Length < 4)
            {
                status.Text = "PIN 码至少需要 4 个字符。";
                dialog.IsPrimaryButtonEnabled = false;
                return;
            }
            if (confirmText.Length > 0 && !string.Equals(pinText, confirmText, StringComparison.Ordinal))
            {
                status.Text = "两次输入的 PIN 码不一致。";
                dialog.IsPrimaryButtonEnabled = false;
                return;
            }
            status.Text = "先验证当前主密码，再设置本地解锁 PIN 码。输错 5 次将自动停用 PIN。";
            dialog.IsPrimaryButtonEnabled = masterPassword.Password.Length != 0 && pinText.Length >= 4 &&
                string.Equals(pinText, confirmText, StringComparison.Ordinal);
        }

        masterPassword.PasswordChanged += (_, _) => UpdateValidity();
        pin.PasswordChanged += (_, _) => UpdateValidity();
        confirmPin.PasswordChanged += (_, _) => UpdateValidity();
        dialog.Opened += (_, _) => masterPassword.Focus(FocusState.Programmatic);
        dialog.Closing += (_, args) =>
        {
            if (_unlockRequest is null) return;
            args.Cancel = true;
            _unlockRequest.Cancel();
        };
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (_unlockRequest is not null || masterPassword.Password.Length == 0 || pin.Password.Length < 4 ||
                !string.Equals(pin.Password, confirmPin.Password, StringComparison.Ordinal)) return;
            var enteredPassword = masterPassword.Password;
            var enteredPin = pin.Password;
            var requireRestart = requireMasterOnRestart.IsChecked == true;
            using var request = _unlockRequest = new CancellationTokenSource();
            masterPassword.Password = string.Empty;
            pin.Password = string.Empty;
            confirmPin.Password = string.Empty;
            masterPassword.IsEnabled = pin.IsEnabled = confirmPin.IsEnabled = requireMasterOnRestart.IsEnabled = false;
            dialog.IsPrimaryButtonEnabled = false;
            status.Text = "正在配置 PIN 解锁…";
            try
            {
                await Task.Run(() => RustCore.EnablePin(account, enteredPassword, enteredPin, requireRestart, request.Token));
                enrolled = true;
            }
            catch (OperationCanceledException) { status.Text = "已取消设置。"; }
            catch (VaultException error) { status.Text = error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error)) { status.Text = "无法加载 Rust 核心，请修复应用安装。"; }
            finally
            {
                enteredPassword = string.Empty;
                enteredPin = string.Empty;
                _unlockRequest = null;
                if (!_closed)
                {
                    if (enrolled || request.IsCancellationRequested) dialog.Hide();
                    else
                    {
                        masterPassword.IsEnabled = pin.IsEnabled = confirmPin.IsEnabled = requireMasterOnRestart.IsEnabled = true;
                        UpdateValidity();
                        masterPassword.Focus(FocusState.Programmatic);
                    }
                }
            }
        };
        try { await dialog.ShowAsync(); }
        finally
        {
            masterPassword.Password = string.Empty;
            pin.Password = string.Empty;
            confirmPin.Password = string.Empty;
        }
        return enrolled;
    }

    private async Task ShowAccountFingerprintAsync(SettingsPage page)
    {
        if (_closed || _signInDialogOpen || _accountTransition || ActiveAccount is not { State: VaultSessionState.Unlocked } account) return;
        _signInDialogOpen = true;
        UpdateSessionControls(VaultSessionState.Unlocked);
        try
        {
            var fingerprint = await Task.Run(RustCore.GetAccountFingerprint);
            if (_closed) return;
            var copyStatus = new TextBlock { TextWrapping = TextWrapping.Wrap, MaxWidth = 360 };
            var phraseBox = new TextBlock
            {
                Text = fingerprint,
                IsTextSelectionEnabled = true,
                TextWrapping = TextWrapping.Wrap,
                FontWeight = FontWeights.SemiBold,
                MaxWidth = 360,
            };
            var dialog = new ContentDialog
            {
                XamlRoot = NavView.XamlRoot,
                Title = "账户指纹短语",
                PrimaryButtonText = "复制指纹",
                CloseButtonText = "关闭",
                DefaultButton = ContentDialogButton.Close,
                Content = new StackPanel
                {
                    Spacing = 10,
                    Children =
                    {
                        new TextBlock
                        {
                            Text = $"账户 {account.Email} 的公钥指纹短语：",
                            TextWrapping = TextWrapping.Wrap,
                            MaxWidth = 360,
                        },
                        phraseBox,
                        copyStatus,
                    },
                },
            };
            dialog.PrimaryButtonClick += (_, args) =>
            {
                args.Cancel = true;
                try
                {
                    ClipboardCleaner.Copy(fingerprint);
                    copyStatus.Text = "已复制账户指纹短语，将在剪贴板超时后自动清除。";
                }
                catch
                {
                    copyStatus.Text = "无法写入剪贴板。";
                }
            };
            await dialog.ShowAsync();
        }
        catch (VaultException error)
        {
            if (!_closed && ActiveAccount?.Id == account.Id && ReferenceEquals(NavFrame.Content, page))
                page.ReportPinUnlock(error.UserMessage);
        }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            if (!_closed && ActiveAccount?.Id == account.Id && ReferenceEquals(NavFrame.Content, page))
                page.ReportPinUnlock("无法加载 Rust 核心，请修复应用安装。");
        }
        finally
        {
            _signInDialogOpen = false;
            if (!_closed)
            {
                UpdateSessionControls(RustCore.SessionState);
                await ApplyPendingWindowsTimeoutAsync();
            }
        }
    }
}
