using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

public sealed partial class SettingsPage
{
    internal Action<VaultTimeoutSettings>? VaultTimeoutChanged { get; set; }
    private VaultTimeoutSettings _currentTimeout;
    private bool _timeoutEnabled;
    private bool _updatingTimeout;

    private void InitializeTimeoutControls()
    {
        foreach (var (mode, text) in new[] {
            (VaultTimeoutMode.Inactivity, "经过指定分钟"),
            (VaultTimeoutMode.SystemIdle, "系统空闲 5 分钟"),
            (VaultTimeoutMode.SystemLock, "系统锁定时"),
            (VaultTimeoutMode.SystemSleep, "系统休眠时"),
            (VaultTimeoutMode.AppRestart, "应用重启时"),
            (VaultTimeoutMode.Never, "从不"),
        }) TimeoutModeBox.Items.Add(new ComboBoxItem { Content = text, Tag = mode });
        TimeoutActionBox.Items.Add(new ComboBoxItem { Content = "锁定", Tag = VaultTimeoutAction.Lock });
        TimeoutActionBox.Items.Add(new ComboBoxItem { Content = "退出登录", Tag = VaultTimeoutAction.LogOut });
        TimeoutMinutesBox.Maximum = VaultTimeoutSettings.MaximumMinutes;
        TimeoutModeBox.SelectionChanged += (_, _) => {
            TimeoutMinutesBox.Visibility = SelectedTimeoutMode == VaultTimeoutMode.Inactivity ? Visibility.Visible : Visibility.Collapsed;
            if (!_updatingTimeout) TimeoutFeedback.Text = "修改后请选择保存。";
        };
        TimeoutActionBox.SelectionChanged += (_, _) => { if (!_updatingTimeout) TimeoutFeedback.Text = "修改后请选择保存。"; };
        ShowVaultTimeout(VaultTimeoutSettings.Default, false, true, true);
    }

    private VaultTimeoutMode SelectedTimeoutMode => TimeoutModeBox.SelectedItem is ComboBoxItem { Tag: VaultTimeoutMode mode } ? mode : VaultTimeoutMode.AppRestart;

    internal void ShowVaultTimeout(VaultTimeoutSettings settings, bool enabled, bool canMonitorLock, bool canMonitorSleep)
    {
        _currentTimeout = settings;
        _timeoutEnabled = enabled;
        _updatingTimeout = true;
        foreach (ComboBoxItem item in TimeoutModeBox.Items) {
            if (item.Tag is not VaultTimeoutMode mode) continue;
            item.IsEnabled = mode != VaultTimeoutMode.SystemLock || canMonitorLock;
            if (mode == VaultTimeoutMode.SystemSleep) item.IsEnabled = canMonitorSleep;
            if (mode == settings.Mode) TimeoutModeBox.SelectedItem = item;
        }
        TimeoutMinutesBox.Value = settings.Minutes;
        foreach (ComboBoxItem item in TimeoutActionBox.Items) if (item.Tag is VaultTimeoutAction action && action == settings.Action) TimeoutActionBox.SelectedItem = item;
        TimeoutModeBox.IsEnabled = TimeoutMinutesBox.IsEnabled = TimeoutActionBox.IsEnabled = SaveTimeoutButton.IsEnabled = enabled;
        var effective = settings.EffectiveMode(canMonitorLock, canMonitorSleep);
        TimeoutFeedback.Text = !enabled ? "登录并解锁后，可修改当前账户的超时设置。" : effective != settings.Mode
            ? effective == VaultTimeoutMode.SystemSleep ? "系统锁定监听不可用，当前按系统休眠触发。" : "系统事件监听不可用，当前按应用重启触发。"
            : string.Empty;
        _updatingTimeout = false;
    }

    internal void ReportTimeoutSaveFailure() => TimeoutFeedback.Text = "设置已应用于本次会话，保存到本机失败。重新登录后请检查此设置。";
    internal void ReportTimeoutSaved() => TimeoutFeedback.Text = "已保存当前账户的超时设置。";

    private async void SaveTimeout_Click(object sender, RoutedEventArgs e)
    {
        if (!_timeoutEnabled || VaultTimeoutChanged is null) return;
        var minutes = TimeoutMinutesBox.Value;
        if (!double.IsFinite(minutes) || minutes < 1 || minutes > int.MaxValue || Math.Floor(minutes) != minutes) {
            TimeoutFeedback.Text = "请填写至少 1 分钟的整数。";
            return;
        }
        var action = TimeoutActionBox.SelectedItem is ComboBoxItem { Tag: VaultTimeoutAction selected } ? selected : VaultTimeoutAction.Lock;
        var settings = new VaultTimeoutSettings(SelectedTimeoutMode, (int)minutes, action);
        if (settings.Mode == VaultTimeoutMode.Never && _currentTimeout.Mode != VaultTimeoutMode.Never) {
            var confirmation = new ContentDialog { XamlRoot = XamlRoot, Title = "保持保险库解锁？",
                Content = "选择「从不」后，本次会话会保持解锁，直到你手动锁定或退出。",
                PrimaryButtonText = "保持解锁", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
            if (await confirmation.ShowAsync() != ContentDialogResult.Primary || !_timeoutEnabled) return;
        }
        VaultTimeoutChanged(settings);
    }
}
