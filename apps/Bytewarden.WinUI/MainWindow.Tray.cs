using Bytewarden.WinUI.Interop;
using Microsoft.UI.Windowing;

namespace Bytewarden.WinUI;

public sealed partial class MainWindow
{
    private WindowsTrayIcon? _tray;
    private bool _exiting;
    private bool _exitPrepared;
    private bool _exitReady;
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

    private async void OnWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (_exitReady) return;
        if (!_exiting && DesktopPreferences.Read(DesktopPreferences.KeepInTray) && TryHideToTray())
        {
            args.Cancel = true;
            return;
        }
        // Keep the dispatcher alive for a bounded cleanup; duplicate close requests must
        // not bypass it. Revoke vault keys/UI immediately, before waiting on Windows.
        args.Cancel = true;
        await ExitAsync();
    }

    private async Task ExitAsync()
    {
        if (_exitPrepared || _closed) return;
        _exiting = true;
        try
        {
            PrepareForExit();
            _ = await ClipboardCleaner.ClearForExitAsync(TimeSpan.FromSeconds(5));
        }
        finally
        {
            _exitReady = true;
            Close();
        }
    }

    private void RestoreIfTrayUnavailable()
    {
        if (_hiddenToTray) ShowMainWindow();
    }

    private async void ExitFromTray()
    {
        if (_closed || _exiting) return;
        await ExitAsync();
    }

    private void DisposeTray()
    {
        AppWindow.Closing -= OnWindowClosing;
        _tray?.Dispose();
        _tray = null;
    }
}
