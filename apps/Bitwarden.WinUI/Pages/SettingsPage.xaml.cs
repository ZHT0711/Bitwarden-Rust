using Microsoft.UI.Xaml.Controls;
using Bitwarden_WinUI.Interop;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace Bitwarden_WinUI.Pages;

public sealed partial class SettingsPage : Page
{
    public SettingsPage()
    {
        InitializeComponent();
        WebsiteIconsToggle.IsOn = WebsiteIcons.Enabled;
        WebsiteIconsToggle.Toggled += (_, _) => WebsiteIcons.SetEnabled(WebsiteIconsToggle.IsOn);
    }
}
