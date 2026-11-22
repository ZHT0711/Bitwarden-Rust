using Bitwarden_WinUI.Interop;
using Microsoft.UI.Windowing;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private WindowsTrayIcon? _tray;
    private bool _exiting;
    private bool _hiddenToTray;

    private void InitializeTray()
    {
        _tray = new WindowsTrayIcon(WinRT.Interop.WindowNative.GetWindowHandle(this), DispatcherQueue,
            ShowMainWindow, ExitFromTray, RestoreIfTrayUnavailable);
        AppWindow.Closing += OnWindowClosing;
    }

    internal void Launch(bool startup)
    {
        if (startup && DesktopPreferences.Read(DesktopPreferences.StartHidden) && TryHideToTray()) return;
        if (DesktopPreferences.Read(DesktopPreferences.KeepInTray)) _tray?.TryRegister();
        ShowMainWindow();
    }

    internal void ShowMainWindow()
    {
        if (_closed || _exiting) return;
        if (_hiddenToTray)
        {
            _hiddenToTray = false;
            AppWindow.Show();
        }
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter)
            presenter.Restore();
        Activate();
    }

    private bool TryHideToTray()
    {
        // Never hide the only window without first making the restore command reachable.
        if (_tray?.TryRegister() != true) return false;
        _hiddenToTray = true;
        AppWindow.Hide();
        return true;
    }

    private void OnWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (!_exiting && DesktopPreferences.Read(DesktopPreferences.KeepInTray) && TryHideToTray())
            args.Cancel = true;
    }

    private void RestoreIfTrayUnavailable()
    {
        if (_hiddenToTray) ShowMainWindow();
    }

    private void ExitFromTray()
    {
        if (_closed || _exiting) return;
        _exiting = true;
        Close();
    }

    private void DisposeTray()
    {
        AppWindow.Closing -= OnWindowClosing;
        _tray?.Dispose();
        _tray = null;
    }
}
