using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;

namespace Bytewarden.WinUI.Pages;

public sealed partial class SettingsPage
{
    internal Func<Task>? ChangeHelloUnlock { get; set; }

    internal void ShowHelloUnlock(VaultAccount? account, bool busy)
    {
        var enabled = account?.HelloEnabled == true;
        HelloUnlockButton.Content = enabled ? "关闭 Windows Hello 解锁" : "启用 Windows Hello 解锁";
        HelloUnlockButton.IsEnabled = !busy && account?.State == VaultSessionState.Unlocked;
        HelloUnlockStatus.Text = account?.State != VaultSessionState.Unlocked
            ? "请先登录并解锁当前账户。"
            : enabled
                ? "此账户可在这台电脑上使用 Windows Hello 解锁，包括应用重启后。仍可使用主密码。"
                : "同步保险库后即可启用。你需要先输入一次主密码，再完成 Windows Hello 验证。";
    }

    internal void ReportHelloUnlock(string message) => HelloUnlockStatus.Text = message;

    private async void HelloUnlock_Click(object sender, RoutedEventArgs e)
    {
        if (ChangeHelloUnlock is not null) await ChangeHelloUnlock();
    }
}
