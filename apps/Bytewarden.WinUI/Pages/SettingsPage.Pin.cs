using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;

namespace Bytewarden.WinUI.Pages;

public sealed partial class SettingsPage
{
    internal Func<Task>? ChangePinUnlock { get; set; }
    internal Func<Task>? ShowFingerprint { get; set; }

    internal void ShowPinUnlock(VaultAccount? account, bool busy)
    {
        var enabled = account?.PinEnabled == true;
        PinUnlockButton.Content = enabled ? "关闭 PIN 解锁" : "启用 PIN 解锁";
        PinUnlockButton.IsEnabled = !busy && account?.State == VaultSessionState.Unlocked;
        ViewFingerprintButton.IsEnabled = !busy && account is { State: VaultSessionState.Unlocked };
        PinUnlockStatus.Text = account?.State != VaultSessionState.Unlocked
            ? "请先登录并解锁当前账户。"
            : enabled
                ? account!.PinPersistent
                    ? "此账户可使用 PIN 解锁（包括应用重启后）。输错 5 次后将自动停用。"
                    : "此账户可在本次会话锁定后使用 PIN 解锁；应用重启后仍需主密码。"
                : "设置本地 PIN 码以便快速解锁保险库。启用前需验证主密码。";
    }

    internal void ReportPinUnlock(string message) => PinUnlockStatus.Text = message;

    private async void PinUnlock_Click(object sender, RoutedEventArgs e)
    {
        if (ChangePinUnlock is not null) await ChangePinUnlock();
    }

    private async void ViewFingerprint_Click(object sender, RoutedEventArgs e)
    {
        if (ShowFingerprint is not null) await ShowFingerprint();
    }
}
