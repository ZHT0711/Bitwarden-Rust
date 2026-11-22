using Bitwarden_WinUI.Interop;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private void ApplyDesktopTheme(int theme) => WindowRoot.RequestedTheme = theme switch
    {
        1 => ElementTheme.Light,
        2 => ElementTheme.Dark,
        _ => ElementTheme.Default,
    };

    private void MinimizeAfterCopy()
    {
        if (!_closed && !_hiddenToTray && !_exiting && DesktopPreferences.Read(DesktopPreferences.MinimizeOnCopy, fallback: false)
            && AppWindow.Presenter is OverlappedPresenter presenter)
            presenter.Minimize();
    }
}
