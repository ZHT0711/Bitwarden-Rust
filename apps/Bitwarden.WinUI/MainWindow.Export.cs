using Bitwarden_WinUI.Controls;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.Storage.Pickers;
using System.Runtime.InteropServices;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private async Task ExportVaultAsync(SettingsPage page)
    {
        if (_closed || _signInDialogOpen || _accountTransition ||
            ActiveAccount is not { State: VaultSessionState.Unlocked, NeedsSignIn: false } account) return;
        _signInDialogOpen = true;
        using var request = _unlockRequest = new CancellationTokenSource();
        UpdateSessionControls(VaultSessionState.Unlocked);
        page.ReportVaultExport(string.Empty);
        VaultExportReport? report = null;
        try { report = await ConfirmVaultExportAsync(account, request); }
        catch (OperationCanceledException) { }
        catch (COMException) { if (!_closed) page.ReportVaultExport("无法显示导出对话框，请重试。"); }
        finally
        {
            _unlockRequest = null;
            _signInDialogOpen = false;
            if (!_closed)
            {
                UpdateSessionControls(RustCore.SessionState);
                await ApplyPendingWindowsTimeoutAsync();
                if (report is { } outcome && ActiveAccount is { State: VaultSessionState.Unlocked } current && current.Id == account.Id)
                    page.ReportVaultExport($"已保存 {outcome.Items} 项、{outcome.Folders} 个文件夹。跳过 {outcome.SkippedRestricted} 个受限条目、{outcome.OmittedUnsupported} 个格式不支持的条目，未包含 {outcome.OmittedAttachments} 个附件文件。");
            }
        }
    }

    private async Task<VaultExportReport?> ConfirmVaultExportAsync(VaultAccount account, CancellationTokenSource request)
    {
        var format = new ComboBox { Header = "导出格式", HorizontalAlignment = HorizontalAlignment.Stretch,
            Items = { "密码保护的 JSON（推荐）", "账户限定的加密 JSON", "明文 JSON", "明文 CSV（仅登录和安全笔记）" }, SelectedIndex = 0 };
        var master = new PasswordBox { Header = "当前账户的主密码" };
        var exportPassword = new PasswordBox { Header = "导出文件密码", MaxLength = 1024 };
        var confirmation = new PasswordBox { Header = "再次输入导出文件密码", MaxLength = 1024 };
        var confirmPlain = new CheckBox { Content = "我了解此文件未加密，仍要导出" };
        var description = Text(string.Empty);
        var feedback = Text(string.Empty);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetLiveSetting(feedback, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        var passwordPanel = new StackPanel { Spacing = 8, Children = { PasswordVisibility.WithToggle(exportPassword), PasswordVisibility.WithToggle(confirmation) } };
        var panel = new StackPanel { Spacing = 9, MaxWidth = 440, Children =
        {
            Text($"仅导出 {account.Email} · {account.ServerLabel} 的个人条目。组织条目、回收站和附件文件不在此导出中。"),
            format, description, confirmPlain, PasswordVisibility.WithToggle(master), passwordPanel, feedback
        } };
        var dialog = new ContentDialog
        {
            XamlRoot = NavView.XamlRoot, Title = "导出个人保险库", Content = new ScrollViewer { Content = panel, MaxHeight = 540 },
            PrimaryButtonText = "选择保存位置…", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close,
        };
        var running = false;
        VaultExportReport? saved = null;
        UpdateFormat();
        format.SelectionChanged += (_, _) => UpdateFormat();
        dialog.Closing += (_, args) =>
        {
            if (!running) return;
            args.Cancel = true;
            request.Cancel();
        };
        using var cancellation = request.Token.Register(() => DispatcherQueue.TryEnqueue(() =>
        {
            master.Password = exportPassword.Password = confirmation.Password = string.Empty;
            if (!running && !_closed) dialog.Hide();
        }));
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (running || request.IsCancellationRequested) return;
            var plaintext = format.SelectedIndex >= 2;
            var csv = format.SelectedIndex == 3;
            var portable = format.SelectedIndex == 0;
            if (master.Password.Length == 0) { feedback.Text = "请输入主密码。"; return; }
            if (plaintext && confirmPlain.IsChecked != true) { feedback.Text = "请先确认明文文件风险。"; return; }
            if (portable && (exportPassword.Password.Length == 0 || exportPassword.Password != confirmation.Password))
            { feedback.Text = "请输入一致的导出文件密码。"; return; }
            running = true;
            dialog.IsPrimaryButtonEnabled = false;
            foreach (var control in new Control[] { format, master, confirmPlain, exportPassword, confirmation }) control.IsEnabled = false;
            feedback.Text = "请选择一个新文件，已有文件不会被覆盖。";
            var selected = format.SelectedIndex switch { 0 => "passwordJson", 1 => "accountJson", 3 => "csv", _ => "plainJson" };
            try
            {
                var extension = csv ? ".csv" : ".json";
                var picker = new FileSavePicker(NavView.XamlRoot.ContentIslandEnvironment.AppWindowId)
                {
                    SuggestedFileName = $"bitwarden-{DateTime.Now:yyyyMMdd-HHmmss}{extension}",
                    SuggestedStartLocation = PickerLocationId.Downloads,
                    ShowOverwritePrompt = false,
                };
                picker.FileTypeChoices.Add(csv ? "Bitwarden CSV" : "Bitwarden JSON", new List<string> { extension });
                var file = await picker.PickSaveFileAsync().AsTask(request.Token);
                if (file is null) return;
                var entered = master.Password;
                var protection = portable ? exportPassword.Password : string.Empty;
                master.Password = exportPassword.Password = confirmation.Password = string.Empty;
                feedback.Text = "正在复验主密码、检查服务器策略并导出…";
                saved = await Task.Run(() => RustCore.ExportVault(account.Id, file.Path, selected, plaintext, entered, protection, request.Token), request.Token);
            }
            catch (OperationCanceledException) { }
            catch (VaultException error)
            {
                feedback.Text = error.Code switch
                {
                    "permissionDenied" => "当前组织策略或账户权限不允许导出此保险库。",
                    "fileAccess" => "无法保存。请选择可写目录和一个不存在的文件名。",
                    "invalidResponse" => "同步数据不完整，为避免无提示地丢失数据，未生成备份。",
                    "exportCsvField" => "部分自定义字段含换行，或字段值含冒号加空格，无法用 Bitwarden CSV 无损表示。未生成文件，请改用 JSON。",
                    _ => error.UserMessage,
                };
            }
            catch (Exception error) when (error is COMException or InvalidOperationException or ArgumentException || IsCoreUnavailable(error))
            { feedback.Text = "无法完成导出，请检查文件选择器和应用安装。"; }
            finally
            {
                running = false;
                if (!_closed)
                {
                    if (saved is not null || request.IsCancellationRequested) dialog.Hide();
                    else
                    {
                        dialog.IsPrimaryButtonEnabled = true;
                        foreach (var control in new Control[] { format, master, confirmPlain, exportPassword, confirmation }) control.IsEnabled = true;
                    }
                }
            }
        };
        try { await dialog.ShowAsync(); }
        finally { master.Password = exportPassword.Password = confirmation.Password = string.Empty; dialog.Content = null; }
        return saved;

        static TextBlock Text(string text) => new() { Text = text, TextWrapping = TextWrapping.Wrap };

        void UpdateFormat()
        {
            passwordPanel.Visibility = format.SelectedIndex == 0 ? Visibility.Visible : Visibility.Collapsed;
            confirmPlain.Visibility = format.SelectedIndex >= 2 ? Visibility.Visible : Visibility.Collapsed;
            confirmPlain.IsChecked = false;
            exportPassword.Password = confirmation.Password = string.Empty;
            description.Text = format.SelectedIndex switch
            {
                0 => "请保存独立的导出密码。丢失此密码后无法恢复备份。",
                1 => "只可导入具有相同加密密钥的账户；账户密钥轮换后，旧备份可能无法使用。",
                3 => "警告：CSV 未加密，只包含登录和安全笔记；不包含通行密钥、密码历史、URI 匹配规则或空文件夹，自定义字段类型会丢失。同名文件夹会合并。它不是完整备份；请优先使用密码保护的 JSON。不要用电子表格打开含不可信内容的 CSV，以免触发公式。",
                _ => "警告：任何能读取此文件的人都能看到密码、验证码密钥、SSH 密钥和通行密钥。请勿共享或保存在同步文件夹中。",
            };
        }
    }
}
