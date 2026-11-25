using System.Globalization;
using Bytewarden.WinUI.Controls;
using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.Windows.Storage.Pickers;
using System.Runtime.InteropServices;

namespace Bytewarden.WinUI.Pages;

public sealed partial class SendsPage
{
    private const string DateFormat = "yyyy-MM-dd HH:mm";
    private static string LocalDate(string? date) => date is null ? "" :
        DateTimeOffset.Parse(date, CultureInfo.InvariantCulture).ToLocalTime().ToString(DateFormat, CultureInfo.InvariantCulture);

    private static bool TryDate(string input, string? current, out string? date)
    {
        date = current;
        if (input == LocalDate(current)) return true;
        if (string.IsNullOrWhiteSpace(input)) { date = null; return true; }
        if (!DateTime.TryParseExact(input.Trim(), DateFormat, CultureInfo.InvariantCulture, DateTimeStyles.None, out var local) ||
            TimeZoneInfo.Local.IsInvalidTime(local) || TimeZoneInfo.Local.IsAmbiguousTime(local)) return false;
        date = new DateTimeOffset(local, TimeZoneInfo.Local.GetUtcOffset(local)).ToString("O", CultureInfo.InvariantCulture);
        return true;
    }

    private static TextBlock Paragraph(string text) => new() { Text = text, TextWrapping = TextWrapping.Wrap };
    private static ScrollViewer DialogScroller(StackPanel panel) => new() { Content = panel, MaxHeight = 540,
        VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };

    private async Task ShowDetailAsync(SendDetail detail)
    {
        var account = _account;
        var panel = new StackPanel { Spacing = 10, MinWidth = 280 };
        var expired = detail.ExpirationDate is not null ? LocalDate(detail.ExpirationDate) : "未单独设置";
        panel.Children.Add(Paragraph($"删除时间：{LocalDate(detail.DeletionDate)}\n过期时间：{expired}\n已访问：{detail.AccessCount} 次" +
            (detail.MaxAccessCount is { } limit ? $"（上限 {limit}）" : "")));
        panel.Children.Add(Paragraph(detail.AuthType switch { 0 => "需要邮箱验证", 1 => "需要密码", _ => "链接没有额外验证" }));
        if (detail.FileName is { Length: > 0 } file) panel.Children.Add(Paragraph($"文件：{file}"));
        if (detail.Text is not null)
        {
            var text = Paragraph(detail.Hidden ? "" : detail.Text);
            text.IsTextSelectionEnabled = false;
            if (detail.Hidden)
            {
                var reveal = new ToggleButton { Content = "显示文本" };
                reveal.Checked += (_, _) => text.Text = detail.Text;
                reveal.Unchecked += (_, _) => text.Text = string.Empty;
                panel.Children.Add(reveal);
            }
            panel.Children.Add(text);
        }
        if (detail.Notes is { Length: > 0 } notes) panel.Children.Add(Paragraph($"私人备注：{notes}"));
        var removeAuth = new Button { Content = "移除链接验证", IsEnabled = !_requiresRefresh,
            Visibility = detail.AuthType != 2 || detail.HasPassword ? Visibility.Visible : Visibility.Collapsed };
        panel.Children.Add(removeAuth);
        var dialog = new ContentDialog { Title = detail.Name, XamlRoot = XamlRoot, Content = DialogScroller(panel),
            PrimaryButtonText = (detail.Kind is 0 or 1) && !_requiresRefresh ? "编辑" : string.Empty,
            CloseButtonText = "关闭", DefaultButton = ContentDialogButton.Close };
        var remove = false;
        removeAuth.Click += (_, _) => { remove = true; dialog.Hide(); };
        var result = await ShowDialogAsync(dialog, () => { panel.Children.Clear(); dialog.Content = null; dialog.Title = null; });
        if (!_online || account != _account) return;
        if (remove) await ConfirmRemoveAuthAsync(detail);
        else if (result == ContentDialogResult.Primary) await ShowEditorAsync(detail);
    }

    private async Task ConfirmRemoveAuthAsync(SendDetail detail)
    {
        var account = _account;
        var dialog = new ContentDialog { Title = "移除链接验证？", XamlRoot = XamlRoot,
            Content = Paragraph("移除密码或邮箱验证后，只要持有链接就可以访问尚有效的 Send。"),
            PrimaryButtonText = "确认移除", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
        if (await ShowDialogAsync(dialog, () => dialog.Content = null) != ContentDialogResult.Primary || account != _account) return;
        var removed = false;
        await RunAsync((id, token) => { RustCore.RemoveSendAuth(id, detail, token); return true; }, ok => removed = ok);
        if (removed) await RefreshAsync();
    }

    private async Task ShowEditorAsync(SendDetail? existing, bool newFile = false)
    {
        if (_requiresRefresh || !_online || _dialog is not null || _work is not null) return;
        var account = _account;
        var isFile = newFile || existing?.Kind == 1;
        using var dialogLifetime = new CancellationTokenSource();
        string? source = null;
        var pickingFile = false;
        var fileLabel = Paragraph(newFile ? "未选择文件（桌面端上限 500 MiB，服务器可能更低）" : existing?.FileName ?? "");
        var pickFile = new Button { Content = "选择要共享的文件…" };
        var cancelUpload = new Button { Content = "取消上传", Visibility = Visibility.Collapsed };
        var name = new TextBox { Header = "名称", Text = existing?.Name ?? "", MaxLength = 1000 };
        var text = new TextBox { Header = "共享文本", Text = existing?.Text ?? "", MaxLength = 1000,
            AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 76 };
        var notes = new TextBox { Header = "私人备注（仅自己可见）", Text = existing?.Notes ?? "", MaxLength = 1000,
            AcceptsReturn = true, TextWrapping = TextWrapping.Wrap, MinHeight = 52 };
        var deletePreset = new ComboBox { Header = "删除时间预设", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (presetLabel, hours) in new (string, int?)[] { ("1 小时后", 1), ("1 天后", 24), ("2 天后", 48), ("3 天后", 72), ("7 天后", 168), ("30 天后", 720), ("自定义", null) })
            deletePreset.Items.Add(new ComboBoxItem { Content = presetLabel, Tag = hours });
        deletePreset.SelectedIndex = existing is null ? 4 : 6;
        var deleted = new TextBox { Header = "删除时间（本地时间）", PlaceholderText = DateFormat,
            Text = existing is null ? DateTime.Now.AddDays(7).ToString(DateFormat, CultureInfo.InvariantCulture) : LocalDate(existing.DeletionDate),
            Visibility = existing is null ? Visibility.Collapsed : Visibility.Visible };
        var expirePreset = new ComboBox { Header = "过期时间预设", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var (presetLabel, hours) in new (string, int?)[] { ("无（跟随删除时间）", 0), ("1 小时后", 1), ("1 天后", 24), ("2 天后", 48), ("3 天后", 72), ("7 天后", 168), ("30 天后", 720), ("自定义", null) })
            expirePreset.Items.Add(new ComboBoxItem { Content = presetLabel, Tag = hours });
        expirePreset.SelectedIndex = existing?.ExpirationDate is null ? 0 : 7;
        var expires = new TextBox { Header = "过期时间（可选，本地时间）", PlaceholderText = DateFormat,
            Text = LocalDate(existing?.ExpirationDate),
            Visibility = existing?.ExpirationDate is null ? Visibility.Collapsed : Visibility.Visible };
        deletePreset.SelectionChanged += (_, _) =>
        {
            if (deletePreset.SelectedItem is ComboBoxItem { Tag: int hours })
            {
                deleted.Text = DateTime.Now.AddHours(hours).ToString(DateFormat, CultureInfo.InvariantCulture);
                deleted.Visibility = Visibility.Collapsed;
            }
            else
            {
                deleted.Visibility = Visibility.Visible;
            }
        };
        expirePreset.SelectionChanged += (_, _) =>
        {
            if (expirePreset.SelectedItem is ComboBoxItem { Tag: int hours })
            {
                expires.Text = hours == 0 ? string.Empty : DateTime.Now.AddHours(hours).ToString(DateFormat, CultureInfo.InvariantCulture);
                expires.Visibility = Visibility.Collapsed;
            }
            else
            {
                expires.Visibility = Visibility.Visible;
            }
        };
        var access = new NumberBox { Header = "最大访问次数（留空不限）", Minimum = 1, Maximum = int.MaxValue,
            Value = existing?.MaxAccessCount is { } count ? count : double.NaN };
        var hideText = new CheckBox { Content = "接收者默认隐藏文本", IsChecked = existing?.Hidden ?? false };
        var hideEmail = new CheckBox { Content = "对接收者隐藏我的邮箱", IsChecked = existing?.HideEmail ?? false };
        var disabled = new CheckBox { Content = "停用此链接", IsChecked = existing?.Disabled ?? false };
        var autoCopyLink = new CheckBox { Content = "保存后自动复制链接", IsChecked = existing is null };
        var auth = new ComboBox { Header = "接收者验证", HorizontalAlignment = HorizontalAlignment.Stretch };
        auth.Items.Add(existing is null ? "无需验证（持有链接即可访问）" : "保留现有验证方式");
        auth.Items.Add("密码");
        auth.Items.Add("邮箱验证码（需要服务器支持）");
        auth.SelectedIndex = 0;
        var password = new PasswordBox { Header = "接收者密码", MaxLength = 1000 };
        var passwordControl = PasswordVisibility.WithToggle(password);
        passwordControl.Visibility = Visibility.Collapsed;
        var emails = new TextBox { Header = "允许接收的邮箱，用逗号或换行分隔", AcceptsReturn = true, MaxLength = 2499,
            Visibility = Visibility.Collapsed, Text = string.Join(", ", existing?.Emails ?? []) };
        auth.SelectionChanged += (_, _) =>
        {
            passwordControl.Visibility = auth.SelectedIndex == 1 ? Visibility.Visible : Visibility.Collapsed;
            emails.Visibility = auth.SelectedIndex == 2 ? Visibility.Visible : Visibility.Collapsed;
            if (auth.SelectedIndex != 1) password.Password = string.Empty;
        };
        var feedback = Paragraph(string.Empty);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetLiveSetting(feedback, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        var panel = new StackPanel { Spacing = 8, MinWidth = 280 };
        panel.Children.Add(name);
        panel.Children.Add(isFile ? fileLabel : text);
        if (newFile) panel.Children.Add(pickFile);
        foreach (var control in new UIElement[] { notes, auth, passwordControl, emails }) panel.Children.Add(control);
        if (!isFile) panel.Children.Add(hideText);
        foreach (var control in new UIElement[] { hideEmail, disabled, autoCopyLink, deletePreset, deleted, expirePreset, expires, access, feedback }) panel.Children.Add(control);
        if (newFile) panel.Children.Add(cancelUpload);
        var kind = isFile ? "文件" : "文本";
        var dialog = new ContentDialog { Title = $"{(existing is null ? "新建" : "编辑")}{kind} Send", XamlRoot = XamlRoot,
            Content = DialogScroller(panel), PrimaryButtonText = newFile ? "加密并上传" : "保存", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
        var saved = false;
        var savedName = string.Empty;
        var previousIds = _items?.Select(item => item.Id).ToHashSet(StringComparer.Ordinal) ?? [];
        pickFile.Click += async (_, _) =>
        {
            if (pickingFile || _work is not null || !AccountIsCurrent()) return;
            pickingFile = true;
            pickFile.IsEnabled = dialog.IsPrimaryButtonEnabled = false;
            try
            {
                var picker = new FileOpenPicker(XamlRoot.ContentIslandEnvironment.AppWindowId);
                picker.FileTypeFilter.Add("*");
                var selected = await picker.PickSingleFileAsync().AsTask(dialogLifetime.Token);
                if (selected is not null && !dialogLifetime.IsCancellationRequested && AccountIsCurrent())
                {
                    source = selected.Path;
                    var fileName = System.IO.Path.GetFileName(source);
                    fileLabel.Text = $"已选择：{fileName}";
                    if (string.IsNullOrWhiteSpace(name.Text)) name.Text = fileName.Length <= 1000 ? fileName : "";
                    feedback.Text = "";
                }
            }
            catch (OperationCanceledException) { }
            catch (Exception error) when (error is COMException or InvalidOperationException or ArgumentException)
            {
                if (!dialogLifetime.IsCancellationRequested) feedback.Text = "无法打开文件选择器，请重试。";
            }
            finally
            {
                pickingFile = false;
                if (!dialogLifetime.IsCancellationRequested && AccountIsCurrent())
                    pickFile.IsEnabled = dialog.IsPrimaryButtonEnabled = true;
            }
        };
        cancelUpload.Click += (_, _) =>
        {
            cancelUpload.IsEnabled = false;
            feedback.Text = "正在取消上传；如果服务器已创建 Send，将尝试删除…";
            _work?.Cancel();
        };
        dialog.Closing += (_, args) => { if (_work is not null && AccountIsCurrent()) args.Cancel = true; };
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (pickingFile || _work is not null || _requiresRefresh || !AccountIsCurrent()) return;
            if (string.IsNullOrWhiteSpace(name.Text) || (!isFile && string.IsNullOrEmpty(text.Text)))
            { feedback.Text = isFile ? "请输入名称。" : "请输入名称和要共享的文本。"; return; }
            var selectedSource = source;
            if (newFile && selectedSource is null) { feedback.Text = "请选择要共享的文件。"; return; }
            if (deletePreset.SelectedItem is ComboBoxItem { Tag: int deleteHours })
                deleted.Text = DateTime.Now.AddHours(deleteHours).ToString(DateFormat, CultureInfo.InvariantCulture);
            if (expirePreset.SelectedItem is ComboBoxItem { Tag: int expireHours })
                expires.Text = expireHours == 0 ? string.Empty : DateTime.Now.AddHours(expireHours).ToString(DateFormat, CultureInfo.InvariantCulture);
            if (!TryDate(deleted.Text, existing?.DeletionDate, out var deletion) || deletion is null ||
                !TryDate(expires.Text, existing?.ExpirationDate, out var expiration))
            { feedback.Text = "请输入有效且不含夏令时歧义的本地时间（年月日 时:分）。"; return; }
            if (!double.IsNaN(access.Value) && (access.Value != Math.Truncate(access.Value) || access.Value < 1 || access.Value > int.MaxValue))
            { feedback.Text = "访问上限须为正整数，或留空。"; return; }
            var newPassword = auth.SelectedIndex == 1 ? password.Password : null;
            var recipients = auth.SelectedIndex == 2 ? emails.Text.Split([',', '\n'], StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries) : null;
            if (newPassword is { Length: 0 } || recipients is { Length: 0 })
            { feedback.Text = "请填写接收者密码或允许的邮箱。"; return; }
            savedName = name.Text.Trim();
            var draft = new SendDraft(name.Text, isFile ? null : text.Text, notes.Text, !isFile && hideText.IsChecked == true, disabled.IsChecked == true,
                hideEmail.IsChecked == true, double.IsNaN(access.Value) ? null : (uint)access.Value, deletion, expiration, newPassword, recipients);
            dialog.IsPrimaryButtonEnabled = false;
            SetFormEnabled(false);
            var working = newFile ? "正在加密并上传文件…" : "正在加密并保存…";
            feedback.Text = working;
            cancelUpload.IsEnabled = true;
            cancelUpload.Visibility = newFile ? Visibility.Visible : Visibility.Collapsed;
            await RunAsync((id, token) =>
            {
                if (newFile) RustCore.CreateFileSend(id, draft, selectedSource!, token);
                else RustCore.SaveSend(id, existing, draft, token);
                return true;
            }, ok => saved = ok, message => feedback.Text = message, acknowledgedWrite: true);
            if (!AccountIsCurrent() || dialogLifetime.IsCancellationRequested) return;
            cancelUpload.Visibility = Visibility.Collapsed;
            if (saved) dialog.Hide();
            else
            {
                if (feedback.Text == working || feedback.Text.StartsWith("正在取消上传", StringComparison.Ordinal))
                    feedback.Text = "操作已取消。";
                SetFormEnabled(true);
                dialog.IsPrimaryButtonEnabled = !_requiresRefresh;
            }
        };
        await ShowDialogAsync(dialog, () =>
        {
            dialogLifetime.Cancel();
            source = null;
            name.Text = text.Text = notes.Text = emails.Text = password.Password = fileLabel.Text = string.Empty;
            panel.Children.Clear();
            dialog.Content = null;
        });
        if (saved && AccountIsCurrent())
        {
            await RefreshAsync();
            if (autoCopyLink.IsChecked == true && AccountIsCurrent())
            {
                var targetId = existing?.Id
                    ?? _items?.FirstOrDefault(item => !previousIds.Contains(item.Id))?.Id
                    ?? _items?.Where(item => item.Name == savedName).OrderByDescending(item => item.Revision).FirstOrDefault()?.Id;
                if (targetId is not null)
                {
                    await RunAsync((id, token) => RustCore.GetSendLink(id, targetId, token), link =>
                    {
                        try { ClipboardCleaner.Copy(link); Notice("已保存并复制 Send 链接", "持有链接的人可能可访问内容，请只发给预期接收者。", InfoBarSeverity.Success); }
                        catch (Exception error) when (error is InvalidOperationException or COMException)
                        { Notice("Send 已保存，但未能复制链接", "剪贴板暂不可用。", InfoBarSeverity.Warning); }
                    });
                }
            }
        }

        bool AccountIsCurrent() => _loaded && _online && _account == account;

        void SetFormEnabled(bool enabled)
        {
            pickFile.IsEnabled = name.IsEnabled = text.IsEnabled = notes.IsEnabled = auth.IsEnabled = password.IsEnabled = emails.IsEnabled = enabled;
            hideText.IsEnabled = hideEmail.IsEnabled = disabled.IsEnabled = autoCopyLink.IsEnabled = deletePreset.IsEnabled = expirePreset.IsEnabled = deleted.IsEnabled = expires.IsEnabled = access.IsEnabled = enabled;
        }
    }
}
