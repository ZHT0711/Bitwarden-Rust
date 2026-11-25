using Microsoft.UI.Xaml.Controls;
using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace Bytewarden.WinUI.Pages;

public sealed partial class SettingsPage : Page
{
    private bool _readingPasskeyState;
    private bool _passkeyMetadataPublished;
    private async void RegisterPasskeys_Click(object sender, RoutedEventArgs e)
    {
        RegisterPasskeysButton.IsEnabled = false;
        try {
            await Task.Run(RustCore.RegisterPasskeys);
            RefreshPasskeyStatus();
        }
        catch (VaultException error) { PasskeyStatus.Text = error.Code.StartsWith("passkeyProvider:", StringComparison.Ordinal)
            ? $"注册未完成。{error.Code[16..]}" : "当前 Windows 版本或应用包身份无法注册通行密钥提供程序。"; }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        { PasskeyStatus.Text = "无法加载 Rust 核心，请重新构建应用。"; }
        finally { RegisterPasskeysButton.IsEnabled = true; }
    }
    private async void OpenPasskeySettings_Click(object sender, RoutedEventArgs e) =>
        await Windows.System.Launcher.LaunchUriAsync(new Uri("ms-settings:passkeys-advancedoptions"));

    internal async void RefreshPasskeyStatus()
    {
        if (_readingPasskeyState) return;
        _readingPasskeyState = true;
        try {
            var state = await Task.Run(RustCore.PasskeyState);
            if (!IsLoaded) return;
            if (state != "enabled") _passkeyMetadataPublished = false;
            else if (!_passkeyMetadataPublished) _passkeyMetadataPublished = await Task.Run(RustCore.SyncPasskeyMetadata);
            PasskeyStatus.Text = state switch {
                "enabled" => "Windows 已启用此通行密钥提供程序。网站请求会打开账户选择与 Windows Hello 验证。",
                "disabled" => "已注册，Windows 尚未启用。请在系统通行密钥设置中打开 Bytewarden (Preview)，并完成系统要求的确认。",
                _ => "尚未注册 Windows 通行密钥提供程序。",
            };
        }
        catch (Exception error) when (error is VaultException or DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        { if (IsLoaded) PasskeyStatus.Text = "无法读取系统通行密钥提供程序状态。"; }
        finally { _readingPasskeyState = false; }
    }

    public SettingsPage()
    {
        InitializeComponent();
        InitializeTimeoutControls();
        InitializeDesktopOptions();
        Loaded += (_, _) => RefreshPasskeyStatus();
        WebsiteIconsToggle.IsOn = WebsiteIcons.Enabled;
        WebsiteIconsToggle.Toggled += (_, _) => WebsiteIcons.SetEnabled(WebsiteIconsToggle.IsOn);
    }
}
