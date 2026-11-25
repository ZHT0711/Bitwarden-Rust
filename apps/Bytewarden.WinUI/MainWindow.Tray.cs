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
    private Windows.Graphics.RectInt32? _lastNormalBounds;
    private bool _windowMaximized;
    private bool _restoreMaximized;

    private void InitializeTray()
    {
        _tray = new WindowsTrayIcon(WinRT.Interop.WindowNative.GetWindowHandle(this), DispatcherQueue,
            ShowMainWindow, ExitFromTray, RestoreIfTrayUnavailable, LockFromTray, LockAllFromTray);
        AppWindow.Closing += OnWindowClosing;
        AppWindow.Changed += OnWindowChanged;
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
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            if (presenter.State == OverlappedPresenterState.Minimized) presenter.Restore();
            if (_restoreMaximized)
            {
                _restoreMaximized = false;
                presenter.Maximize();
            }
        }
        Activate();
    }

    private bool TryHideToTray()
    {
        // Never hide the only window without first making the restore command reachable.
        if (_tray?.TryRegister() != true) return false;
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            if (presenter.State == OverlappedPresenterState.Restored)
            {
                _lastNormalBounds = new Windows.Graphics.RectInt32(
                    AppWindow.Position.X, AppWindow.Position.Y, AppWindow.Size.Width, AppWindow.Size.Height);
                _windowMaximized = false;
            }
            else if (presenter.State == OverlappedPresenterState.Maximized)
            {
                _windowMaximized = true;
            }
        }
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

    private void OnWindowChanged(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if (sender.Presenter is not OverlappedPresenter presenter) return;
        if (args.DidPresenterChange || args.DidPositionChange || args.DidSizeChange)
        {
            if (presenter.State == OverlappedPresenterState.Maximized)
            {
                _windowMaximized = true;
            }
            else if (presenter.State == OverlappedPresenterState.Restored)
            {
                _windowMaximized = false;
                if (sender.IsVisible)
                {
                    _lastNormalBounds = new Windows.Graphics.RectInt32(
                        sender.Position.X, sender.Position.Y, sender.Size.Width, sender.Size.Height);
                }
            }
        }
        if (args.DidPresenterChange
            && presenter.State == OverlappedPresenterState.Minimized
            && !_hiddenToTray
            && !_exiting
            && !_closed
            && DesktopPreferences.Read(DesktopPreferences.MinimizeToTray, fallback: false))
        {
            TryHideToTray();
        }
    }

    private async void LockFromTray()
    {
        if (_closed || _exiting || ActiveAccount is not { State: VaultSessionState.Unlocked }) return;
        await ChangeAccountsAsync(() => { RustCore.Lock(); return RustCore.GetAccounts(); });
    }

    private async void LockAllFromTray()
    {
        if (_closed || _exiting) return;
        var targets = _accounts
            .Where(account => account.State == VaultSessionState.Unlocked)
            .Select(account => new VaultAccountTimeout(account.Id, LogOut: false))
            .ToArray();
        if (targets.Length == 0) return;
        await ChangeAccountsAsync(() => RustCore.ApplyAccountTimeouts(targets, interrupt: true));
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
        AppWindow.Changed -= OnWindowChanged;
        _tray?.Dispose();
        _tray = null;
    }
}
