using Bitwarden_WinUI.Interop;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    private void WindowRoot_SizeChanged(object sender, SizeChangedEventArgs args)
    {
        if (AccountCommands is null || SearchBox is null) return;
        var compact = args.NewSize.Width < 700;
        Grid.SetRow(AccountCommands, compact ? 1 : 0);
        Grid.SetColumn(AccountCommands, compact ? 0 : 1);
        Grid.SetColumnSpan(AccountCommands, compact ? 2 : 1);
        Grid.SetColumnSpan(SearchBox, compact ? 2 : 1);
        SearchBox.HorizontalAlignment = compact ? HorizontalAlignment.Stretch : HorizontalAlignment.Left;
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
