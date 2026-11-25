using System.Runtime.InteropServices;
using Bytewarden.WinUI.Interop;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Graphics;

namespace Bytewarden.WinUI;

public sealed partial class MainWindow
{
    private const uint WdaNone = 0x0;
    private const uint WdaExcludeFromCapture = 0x11;

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool SetWindowDisplayAffinity(nint hwnd, uint affinity);

    private double _windowWidth = 1200;

    private void WindowRoot_SizeChanged(object sender, SizeChangedEventArgs args)
    {
        _windowWidth = args.NewSize.Width;
        ApplyResponsiveChrome();
    }

    private void ApplyResponsiveChrome()
    {
        if (AccountCommands is null || SearchBox is null) return;
        var compactButtons = _windowWidth < 760;
        var showServerBadge = _windowWidth >= 960;
        var signedIn = ActiveAccount is not null;

        SyncButtonLabel.Visibility = compactButtons ? Visibility.Collapsed : Visibility.Visible;
        LockButtonLabel.Visibility = compactButtons ? Visibility.Collapsed : Visibility.Visible;
        SyncButton.Padding = compactButtons ? new Thickness(8, 0, 8, 0) : new Thickness(10, 0, 10, 0);
        LockButton.Padding = compactButtons ? new Thickness(8, 0, 8, 0) : new Thickness(10, 0, 10, 0);

        AccountPrimaryLabel.Visibility = compactButtons && signedIn ? Visibility.Collapsed : Visibility.Visible;
        AccountServerBadge.Visibility = showServerBadge && signedIn ? Visibility.Visible : Visibility.Collapsed;
        SignInButton.Padding = compactButtons && signedIn ? new Thickness(8, 0, 8, 0) : new Thickness(10, 0, 10, 0);
    }

    private void ApplyPreventScreenshots(bool enabled)
    {
        try
        {
            SetWindowDisplayAffinity(
                WinRT.Interop.WindowNative.GetWindowHandle(this),
                enabled ? WdaExcludeFromCapture : WdaNone);
        }
        catch (EntryPointNotFoundException) { }
    }

    private void RestoreWindowBounds()
    {
        try
        {
            if (DesktopPreferences.ReadWindowBounds() is not { } saved) return;
            var rect = new RectInt32(saved.X, saved.Y, saved.Width, saved.Height);
            var workArea = DisplayArea.GetFromRect(rect, DisplayAreaFallback.Nearest).WorkArea;
            var width = Math.Clamp(rect.Width, 320, Math.Max(320, workArea.Width));
            var height = Math.Clamp(rect.Height, 240, Math.Max(240, workArea.Height));
            var x = Math.Clamp(rect.X, workArea.X, Math.Max(workArea.X, workArea.X + workArea.Width - width));
            var y = Math.Clamp(rect.Y, workArea.Y, Math.Max(workArea.Y, workArea.Y + workArea.Height - height));
            var clamped = new RectInt32(x, y, width, height);
            AppWindow.MoveAndResize(clamped);
            _lastNormalBounds = clamped;
            _windowMaximized = saved.Maximized;
            _restoreMaximized = saved.Maximized;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { }
    }

    private void SaveWindowBounds()
    {
        try
        {
            if (AppWindow.Presenter is OverlappedPresenter presenter)
            {
                if (_lastNormalBounds is null && presenter.State == OverlappedPresenterState.Restored)
                {
                    _lastNormalBounds = new RectInt32(
                        AppWindow.Position.X, AppWindow.Position.Y, AppWindow.Size.Width, AppWindow.Size.Height);
                }
                if (AppWindow.IsVisible)
                    _windowMaximized = presenter.State == OverlappedPresenterState.Maximized;
            }
            if (_lastNormalBounds is { } bounds)
                DesktopPreferences.TrySaveWindowBounds(bounds.X, bounds.Y, bounds.Width, bounds.Height, _windowMaximized);
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { }
    }

    private void ApplyDesktopTheme(int theme) => WindowRoot.RequestedTheme = theme switch
    {
        1 => ElementTheme.Light,
        2 => ElementTheme.Dark,
        _ => ElementTheme.Default,
    };

    private void SearchShortcut_Invoked(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        if (_closed || _signInDialogOpen || _accountTransition || !SearchBox.IsEnabled) return;
        SearchBox.Focus(FocusState.Programmatic);
    }

    private void PrepareForExit()
    {
        if (_exitPrepared) return;
        _exitPrepared = true;
        _closed = true; // Logical shutdown: late account/dialog continuations must not reauthorize UI.
        SaveWindowBounds();
        WindowRoot.IsHitTestVisible = false;
        NavView.IsEnabled = false;
        AccountSettingsScope.SignedOut -= Pages.GeneratorPage.ResetSavedAlgorithm;
        _passkeyTimer?.Stop();
        ClearPasskeySensitiveState();
        _vaultTimeoutTimer?.Stop();
        _unlockRequest?.Cancel();
        if (NavFrame.Content is Pages.HomePage page) page.ClearSensitiveState();
        if (NavFrame.Content is Pages.FolderManagementPage folders) folders.ClearSensitiveState();
        if (NavFrame.Content is Pages.SendsPage sends) sends.ClearSensitiveState();
        if (NavFrame.Content is Pages.GeneratorPage generator) generator.ClearSensitiveState();
        WebsiteIcons.ClearCache();
        ClipboardCleaner.Copied -= MinimizeAfterCopy;
        try
        {
            RustCore.CancelActiveRequest();
            _ = ApplyRestartTimeouts(RustCore.GetAccounts());
        }
        catch (Exception error) when (error is VaultException || IsCoreUnavailable(error)) { }
        finally
        {
            try { RustCore.Shutdown(); } catch (Exception error) when (IsCoreUnavailable(error)) { }
            _windowsVaultEvents?.Dispose();
        }
    }

    private void MinimizeAfterCopy()
    {
        if (!_closed && !_hiddenToTray && !_exiting && DesktopPreferences.Read(DesktopPreferences.MinimizeOnCopy, fallback: false)
            && AppWindow.Presenter is OverlappedPresenter presenter)
            presenter.Minimize();
    }
}
