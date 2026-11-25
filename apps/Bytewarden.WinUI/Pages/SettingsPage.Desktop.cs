using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Runtime.InteropServices;
using Windows.ApplicationModel;

namespace Bytewarden.WinUI.Pages;

public sealed partial class SettingsPage
{
    private bool _updatingDesktopOptions = true;
    private bool _updatingStartup;
    internal Action<int>? ApplyTheme { get; set; }
    internal Action<bool>? ApplyPreventScreenshots { get; set; }

    private void InitializeDesktopOptions()
    {
        KeepInTrayToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.KeepInTray);
        StartHiddenToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.StartHidden);
        MinimizeToTrayToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.MinimizeToTray, fallback: false);
        ThemeChoice.SelectedIndex = DesktopPreferences.ReadTheme();
        QuickCopyActionsToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.ShowQuickCopyActions, fallback: true);
        foreach (var seconds in DesktopPreferences.ClipboardChoices)
            ClipboardDelayChoice.Items.Add(new ComboBoxItem { Content = seconds == 0 ? "不定时清除" : $"{seconds} 秒", Tag = seconds });
        SelectSavedClipboardDelay();
        MinimizeOnCopyToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.MinimizeOnCopy, fallback: false);
        PreventScreenshotsToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.PreventScreenshots, fallback: false);
        AutoPromptHelloToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.AutoPromptHello, fallback: false);
        _updatingDesktopOptions = false;
        Loaded += async (_, _) => await RefreshStartupAsync();
    }

    private void ThemeChoice_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_updatingDesktopOptions || ThemeChoice.SelectedIndex < 0) return;
        var theme = ThemeChoice.SelectedIndex;
        if (DesktopPreferences.TrySaveTheme(theme))
        {
            ApplyTheme?.Invoke(theme);
            ThemeStatus.Text = string.Empty;
            return;
        }
        _updatingDesktopOptions = true;
        ThemeChoice.SelectedIndex = DesktopPreferences.ReadTheme();
        _updatingDesktopOptions = false;
        ThemeStatus.Text = "无法保存应用主题。";
    }

    private void SelectSavedClipboardDelay()
    {
        var saved = DesktopPreferences.ReadClipboardDelay();
        ClipboardDelayChoice.SelectedItem = ClipboardDelayChoice.Items.OfType<ComboBoxItem>().Single(item => (int)item.Tag == saved);
    }

    private void ClipboardDelay_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_updatingDesktopOptions || ClipboardDelayChoice.SelectedItem is not ComboBoxItem { Tag: int seconds }) return;
        if (DesktopPreferences.TrySaveClipboardDelay(seconds)) { ClipboardStatus.Text = string.Empty; return; }
        _updatingDesktopOptions = true;
        SelectSavedClipboardDelay();
        _updatingDesktopOptions = false;
        ClipboardStatus.Text = "无法保存剪贴板设置。";
    }

    private void MinimizeOnCopy_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingDesktopOptions) return;
        if (DesktopPreferences.TrySave(DesktopPreferences.MinimizeOnCopy, MinimizeOnCopyToggle.IsOn))
        { ClipboardStatus.Text = string.Empty; return; }
        _updatingDesktopOptions = true;
        MinimizeOnCopyToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.MinimizeOnCopy, fallback: false);
        _updatingDesktopOptions = false;
        ClipboardStatus.Text = "无法保存复制后最小化选项。";
    }

    private void QuickCopyActions_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingDesktopOptions) return;
        if (DesktopPreferences.TrySave(DesktopPreferences.ShowQuickCopyActions, QuickCopyActionsToggle.IsOn))
        { ThemeStatus.Text = string.Empty; return; }
        _updatingDesktopOptions = true;
        QuickCopyActionsToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.ShowQuickCopyActions, fallback: true);
        _updatingDesktopOptions = false;
        ThemeStatus.Text = "无法保存快捷复制选项。";
    }

    private void PreventScreenshots_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingDesktopOptions) return;
        var enabled = PreventScreenshotsToggle.IsOn;
        if (DesktopPreferences.TrySave(DesktopPreferences.PreventScreenshots, enabled))
        {
            ApplyPreventScreenshots?.Invoke(enabled);
            ClipboardStatus.Text = string.Empty;
            return;
        }
        _updatingDesktopOptions = true;
        PreventScreenshotsToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.PreventScreenshots, fallback: false);
        _updatingDesktopOptions = false;
        ClipboardStatus.Text = "无法保存防截屏选项。";
    }

    private void AutoPromptHello_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingDesktopOptions) return;
        if (DesktopPreferences.TrySave(DesktopPreferences.AutoPromptHello, AutoPromptHelloToggle.IsOn)) return;
        _updatingDesktopOptions = true;
        AutoPromptHelloToggle.IsOn = DesktopPreferences.Read(DesktopPreferences.AutoPromptHello, fallback: false);
        _updatingDesktopOptions = false;
        HelloUnlockStatus.Text = "无法保存自动弹出 Windows Hello 设置。";
    }

    internal Task RefreshStartupAsync() => UpdateStartupAsync(null);

    private async void StartupToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_updatingStartup && IsLoaded) await UpdateStartupAsync(StartupToggle.IsOn);
    }

    private void DesktopOption_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingDesktopOptions || sender is not ToggleSwitch toggle) return;
        var key = ReferenceEquals(toggle, KeepInTrayToggle)
            ? DesktopPreferences.KeepInTray
            : ReferenceEquals(toggle, MinimizeToTrayToggle)
                ? DesktopPreferences.MinimizeToTray
                : DesktopPreferences.StartHidden;
        var fallback = !ReferenceEquals(toggle, MinimizeToTrayToggle);
        if (DesktopPreferences.TrySave(key, toggle.IsOn)) return;
        _updatingDesktopOptions = true;
        toggle.IsOn = DesktopPreferences.Read(key, fallback);
        _updatingDesktopOptions = false;
        StartupStatus.Text = "无法保存本机偏好，请重试。";
    }

    private async Task UpdateStartupAsync(bool? enable)
    {
        if (_updatingStartup || !IsLoaded) return;
        _updatingStartup = true;
        StartupToggle.IsEnabled = false;
        try
        {
            var task = await StartupTask.GetAsync(DesktopPreferences.StartupTaskId);
            if (!IsLoaded) return;
            if (enable == true && task.State == StartupTaskState.Disabled) await task.RequestEnableAsync();
            else if (enable == false && task.State == StartupTaskState.Enabled) task.Disable();
            if (!IsLoaded) return;
            var state = task.State;
            StartupToggle.IsOn = state is StartupTaskState.Enabled or StartupTaskState.EnabledByPolicy;
            StartupToggle.IsEnabled = state is StartupTaskState.Enabled or StartupTaskState.Disabled;
            StartupStatus.Text = state switch
            {
                StartupTaskState.Enabled => "已启用，下次登录 Windows 时启动。",
                StartupTaskState.DisabledByUser => "已在 Windows 中关闭。请在「设置 → 应用 → 启动」中重新启用。",
                StartupTaskState.DisabledByPolicy or StartupTaskState.EnabledByPolicy => "此项由 Windows 策略管理。",
                _ => "未启用 Windows 自启动。",
            };
        }
        catch (Exception error) when (error is COMException or InvalidOperationException or ArgumentException or UnauthorizedAccessException)
        {
            if (IsLoaded)
            {
                StartupToggle.IsOn = false;
                StartupStatus.Text = "无法读取 Windows 自启动状态，请确认正在运行已注册的应用包。";
            }
        }
        finally { _updatingStartup = false; }
    }
}
