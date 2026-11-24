using System.Runtime.InteropServices;
using Bitwarden_WinUI.Controls;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.Storage.Pickers;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private async Task ImportVaultAsync(SettingsPage page)
    {
        if (_closed || _signInDialogOpen || _accountTransition ||
            ActiveAccount is not { State: VaultSessionState.Unlocked, NeedsSignIn: false } account) return;
        _signInDialogOpen = true;
        using var request = _unlockRequest = new CancellationTokenSource();
        UpdateSessionControls(VaultSessionState.Unlocked);
        page.ReportVaultExport(string.Empty);
        try
        {
            var picker = new FileOpenPicker(NavView.XamlRoot.ContentIslandEnvironment.AppWindowId);
            picker.FileTypeFilter.Add(".json");
            picker.FileTypeFilter.Add(".csv");
            var selection = await picker.PickSingleFileAsync().AsTask(request.Token);
            if (selection is not null && !_closed && !request.IsCancellationRequested)
                await ConfirmVaultImportAsync(page, account, selection.Path, request);
        }
        catch (OperationCanceledException) { }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        { if (!_closed) page.ReportVaultExport("无法打开导入对话框，请重试。"); }
        finally
        {
            _unlockRequest = null;
            _signInDialogOpen = false;
            if (!_closed)
            {
                UpdateSessionControls(RustCore.SessionState);
                await ApplyPendingWindowsTimeoutAsync();
            }
        }
    }

    private async Task ConfirmVaultImportAsync(SettingsPage page, VaultAccount account, string source, CancellationTokenSource request)
    {
        var csv = string.Equals(System.IO.Path.GetExtension(source), ".csv", StringComparison.OrdinalIgnoreCase);
        var password = new PasswordBox { Header = "导出文件密码（仅密码保护的 JSON 需要）", MaxLength = 1024 };
        var passwordControl = PasswordVisibility.WithToggle(password);
        passwordControl.Visibility = csv ? Visibility.Collapsed : Visibility.Visible;
        var notice = new TextBlock { TextWrapping = TextWrapping.Wrap };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetLiveSetting(notice, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        var panel = new StackPanel { Spacing = 10, MaxWidth = 425, Children =
        {
            new TextBlock { Text = $"将「{System.IO.Path.GetFileName(source)}」导入 {account.Email} · {account.ServerLabel} 的个人保险库。导入会新增条目和文件夹，不覆盖旧条目，重复导入会产生副本。", TextWrapping = TextWrapping.Wrap },
            passwordControl,
            new TextBlock { Text = "仅支持个人 Bitwarden JSON / CSV，不支持组织或其他产品的 CSV。CSV 只保留登录、安全笔记及其字段文本；同名文件夹会合并。", TextWrapping = TextWrapping.Wrap },
            notice
        } };
        var dialog = new ContentDialog { XamlRoot = NavView.XamlRoot, Title = "确认导入 Bitwarden JSON / CSV", Content = panel,
            PrimaryButtonText = "导入", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
        var running = false;
        dialog.Closing += (_, args) => { if (running) { args.Cancel = true; request.Cancel(); } };
        using var cancellation = request.Token.Register(() => DispatcherQueue.TryEnqueue(() =>
        {
            password.Password = string.Empty;
            if (!running && !_closed) dialog.Hide();
        }));
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (running || request.IsCancellationRequested) return;
            running = true;
            var entered = password.Password;
            password.Password = string.Empty;
            dialog.IsPrimaryButtonEnabled = password.IsEnabled = false;
            notice.Text = "正在检查文件并加密，随后提交到服务器…";
            var completed = false;
            try
            {
                await Task.Run(() => RustCore.ImportVault(account.Id, source, entered, request.Token), request.Token);
                completed = true;
                if (!_closed) page.ReportVaultExport("导入已完成，已同步并确认新增条目和文件夹。");
            }
            catch (OperationCanceledException) { }
            catch (VaultException error)
            {
                var message = error.Code switch
                {
                    "importFormat" => "不支持此文件，或文件中的条目、文件夹关系不完整。未提交导入。",
                    "importPassword" => "无法解密该文件。请检查文件密码，或确认账户限定备份属于当前账户密钥。",
                    "fileAccess" => "无法读取所选文件，或文件超过 32 MiB。",
                    "permissionDenied" => "当前组织策略限制了文件中的条目类型。未提交导入。",
                    "writeUncertain" => "服务器可能已导入部分或全部数据。请先同步并检查保险库，不要直接重试。",
                    "notSynced" => "请先同步个人保险库，再尝试导入。",
                    _ => error.UserMessage,
                };
                notice.Text = message;
                if (error.Code == "writeUncertain")
                {
                    completed = true;
                    if (!_closed) page.ReportVaultExport(message);
                }
            }
            catch (Exception error) when (IsCoreUnavailable(error))
            { notice.Text = "无法加载 Rust 核心，请修复应用安装。"; }
            finally
            {
                entered = string.Empty;
                running = false;
                if (!_closed)
                {
                    if (completed || request.IsCancellationRequested) dialog.Hide();
                    else dialog.IsPrimaryButtonEnabled = password.IsEnabled = true;
                }
            }
        };
        try { await dialog.ShowAsync(); }
        finally { password.Password = string.Empty; dialog.Content = null; }
    }
}
