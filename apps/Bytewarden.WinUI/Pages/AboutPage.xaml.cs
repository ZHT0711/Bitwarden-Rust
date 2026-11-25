using System.Runtime.InteropServices;
using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Windows.ApplicationModel;

namespace Bytewarden.WinUI.Pages;

public sealed partial class AboutPage : Page
{
    public AboutPage()
    {
        InitializeComponent();
        try
        {
            var version = Package.Current.Id.Version;
            AppVersionText.Text = $"桌面客户端版本：{version.Major}.{version.Minor}.{version.Build}.{version.Revision}";
        }
        catch
        {
            AppVersionText.Text = "桌面客户端版本：无程序包版本";
        }
        try
        {
            CoreVersionText.Text = $"Rust 安全核心版本：{RustCore.Version}";
        }
        catch
        {
            CoreVersionText.Text = "Rust 安全核心版本：未加载";
        }
        EnvironmentText.Text = $"运行环境：Windows {Environment.OSVersion.Version} / {RuntimeInformation.ProcessArchitecture}";
    }

    private void CopyVersionInfo_Click(object sender, RoutedEventArgs args)
    {
        // These three lines contain build and platform data only, never vault or account data.
        var details = string.Join(Environment.NewLine, AppVersionText.Text, CoreVersionText.Text, EnvironmentText.Text);
        try
        {
            ClipboardCleaner.Copy(details, allowMinimize: false);
            CopyStatusText.Text = "已复制版本信息。";
        }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            CopyStatusText.Text = "无法访问剪贴板，请稍后重试。";
        }
        FrameworkElementAutomationPeer.FromElement(CopyStatusText)?.RaiseAutomationEvent(AutomationEvents.LiveRegionChanged);
    }
}
