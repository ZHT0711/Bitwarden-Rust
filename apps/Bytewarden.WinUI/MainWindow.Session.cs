using Bytewarden.WinUI.Interop;
using Bytewarden.WinUI.Pages;
using Bytewarden.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Bytewarden.WinUI;

public sealed partial class MainWindow
{
    private bool _accountTransition;
    private bool _closed;
    private CancellationTokenSource? _unlockRequest;
    private VaultAccount[] _accounts = [];
    private VaultAccount? _activeAccount;
    private readonly Dictionary<string, string> _reconnectSyncErrors = new();
    private const string ReconnectSyncWarning = "已登录，尚未同步保险库";
    private VaultAccount? ActiveAccount => _activeAccount;

    private void ApplyAccountSnapshot(VaultAccount[] accounts)
    {
        var previous = _activeAccount;
        var removed = _accounts.Length == 0 ? [] : _accounts.Where(old => accounts.All(current => current.Id != old.Id)).ToArray();
        RetainAccountTimeouts(accounts);
        _accounts = accounts;
        VaultAccount? active = null;
        foreach (var candidate in accounts)
        {
            if (candidate.Active) { active = candidate; break; }
        }
        _activeAccount = active;
        AccountSettingsScope.Use(active?.Scope);
        foreach (var account in removed)
        {
            _reconnectSyncErrors.Remove(account.Id);
            AccountSettingsScope.NotifySignedOut(account.Scope);
        }
        if (previous?.Id == active?.Id) return;
        if (AccountWarning.Title == ReconnectSyncWarning) AccountWarning.IsOpen = false;
        if (active is not null && _reconnectSyncErrors.TryGetValue(active.Id, out var error))
            WarnAboutAccountStorage(ReconnectSyncWarning, error);
        if (active is null) WebsiteIcons.ClearServer();
        else WebsiteIcons.SetServer(active.ServerMode, active.ServerUrl);
        if (NavFrame.Content is GeneratorPage generator) generator.RefreshSettingsScope();
        RecordVaultActivity();
    }

    private void UpdateSessionControls(VaultSessionState state)
    {
        var unlocked = state == VaultSessionState.Unlocked;
        var signedIn = state != VaultSessionState.SignedOut;
        var available = !_accountTransition && !_signInDialogOpen;
        var account = ActiveAccount;
        SearchBox.IsEnabled = unlocked && available;
        SyncButton.IsEnabled = unlocked && available;
        var syncLabel = account is { NeedsSignIn: true } ? "联机同步" : "同步";
        SyncButtonLabel.Text = syncLabel;
        AutomationProperties.SetName(SyncButton, syncLabel);
        ToolTipService.SetToolTip(SyncButton, $"{syncLabel}（Ctrl+R）");

        LockButton.IsEnabled = SignInButton.IsEnabled = available;
        LockButton.Visibility = signedIn ? Visibility.Visible : Visibility.Collapsed;
        var lockLabel = unlocked ? "锁定" : "解锁";
        LockButtonLabel.Text = lockLabel;
        LockButtonIcon.Glyph = unlocked ? "\uE72E" : "\uE785";
        AutomationProperties.SetName(LockButton, lockLabel);
        ToolTipService.SetToolTip(LockButton, $"{lockLabel}（Ctrl+L）");

        AccountPrimaryLabel.Text = account is null ? "登录" : account.Email;
        AccountServerLabel.Text = account is null ? string.Empty : account.CompactServerLabel;
        AutomationProperties.SetName(SignInButton, account is null
            ? "登录" : $"账户 {account.Email} · {account.ServerLabel}");
        ToolTipService.SetToolTip(SignInButton, account is null ? null : $"{account.Email}\n{account.ServerLabel}");
        ApplyResponsiveChrome();
        if (NavFrame.Content is SettingsPage settings)
        {
            var busy = _signInDialogOpen || _accountTransition;
            settings.ShowHelloUnlock(account, busy);
            settings.ShowPinUnlock(account, busy);
            settings.ShowVaultExport(account, busy);
        }
        UpdateVaultTimeoutMonitor();
    }

    private async void Lock_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            if (RustCore.SessionState == VaultSessionState.Locked)
            {
                await ShowUnlockDialogAsync();
            }
            else if (RustCore.SessionState == VaultSessionState.Unlocked)
            {
                await ChangeAccountsAsync(() => { RustCore.Lock(); return RustCore.GetAccounts(); });
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }

    private void ClearActiveAccountContents(bool cancelPasskeys)
    {
        if (cancelPasskeys) ClearPasskeySensitiveState();
        SearchBox.Text = string.Empty;
        _folderToOpen = null;
        NavFrame.BackStack.Clear();
        NavFrame.ForwardStack.Clear();
        if (NavFrame.Content is HomePage vault) vault.ClearSensitiveState();
        if (NavFrame.Content is FolderManagementPage folders) folders.ClearSensitiveState();
        if (NavFrame.Content is SendsPage sends) sends.ClearSensitiveState();
        if (NavFrame.Content is GeneratorPage generator) generator.ClearSensitiveState();
        WebsiteIcons.ClearCache();
        ClipboardCleaner.ClearOwned();
    }

    private async Task ChangeAccountsAsync(Func<VaultAccount[]> change, bool clearActive = true)
    {
        if (_accountTransition || _signInDialogOpen) return;
        _accountTransition = true;
        UpdateSessionControls(RustCore.SessionState);
        if (clearActive)
        {
            NavView.IsEnabled = false;
            ClearActiveAccountContents(cancelPasskeys: true);
        }

        try
        {
            var accounts = await Task.Run(change);
            if (!_closed) ApplyAccountSnapshot(accounts);
        }
        catch (VaultException error) when (error.Code == "cacheRevocationFailed")
        {
            // A failed disk cleanup may still have locked or removed accounts in memory.
            var accounts = await Task.Run(RustCore.GetAccounts);
            if (!_closed) ApplyAccountSnapshot(accounts);
            throw;
        }
        finally
        {
            _accountTransition = false;
            if (!_closed)
            {
                NavView.IsEnabled = true;
                var state = RustCore.SessionState;
                UpdateSessionControls(state);
                if (!await ApplyPendingWindowsTimeoutAsync() && clearActive)
                    await ShowActiveVaultAsync(state, forceSync: false);
            }
        }
    }

    private async Task ShowActiveVaultAsync(VaultSessionState state, bool forceSync, bool allowSyncFallback = true)
    {
        var accountId = ActiveAccount?.Id;
        if (state == VaultSessionState.Unlocked)
        {
            try { await Task.Run(RustCore.SyncPasskeyMetadata); }
            catch (VaultException) { }
        }
        if (_closed || _accountTransition || state != RustCore.SessionState || accountId != ActiveAccount?.Id) return;
        if (NavFrame.Content is HomePage page)
        {
            if (state == VaultSessionState.Locked) page.ShowLocked();
            else if (state == VaultSessionState.SignedOut) page.ShowLoggedOut();
            else await page.RefreshAsync(forceSync, allowSyncFallback);
        }
        else if (NavFrame.Content is FolderManagementPage folders)
        {
            await folders.RefreshAsync(forceSync, allowSyncFallback);
        }
        else if (NavFrame.Content is SendsPage sends)
        {
            await sends.ShowAccountAsync(ActiveAccount, SearchBox.Text);
        }
    }

    private void ReportReconnectSyncError(string accountId, string error)
    {
        var message = error + " 点击「同步」重试，无需再次输入主密码。";
        _reconnectSyncErrors[accountId] = message;
        WarnAboutAccountStorage(ReconnectSyncWarning, message);
    }

    private async Task RetryReconnectSyncAsync(VaultAccount account)
    {
        if (_closed || _signInDialogOpen || _accountTransition) return;
        using var request = _unlockRequest = new CancellationTokenSource();
        _accountTransition = true;
        NavView.IsEnabled = false;
        UpdateSessionControls(VaultSessionState.Unlocked);
        var synced = false;
        try
        {
            await Task.Run(() => RustCore.SyncVault(request.Token));
            _reconnectSyncErrors.Remove(account.Id);
            if (!_closed && AccountWarning.Title == ReconnectSyncWarning) AccountWarning.IsOpen = false;
            synced = true;
        }
        catch (OperationCanceledException) { }
        catch (VaultException error) { if (!_closed) ReportReconnectSyncError(account.Id, error.UserMessage); }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
        finally
        {
            _unlockRequest = null;
            _accountTransition = false;
            if (!_closed)
            {
                NavView.IsEnabled = true;
                var state = RustCore.SessionState;
                UpdateSessionControls(state);
                if (!await ApplyPendingWindowsTimeoutAsync() && synced && account.Id == ActiveAccount?.Id)
                    await ShowActiveVaultAsync(state, forceSync: false);
            }
        }
    }

    private void ShowAccountMenu()
    {
        var menu = new MenuFlyout();
        foreach (var account in _accounts)
        {
            var lockLabel = account.State == VaultSessionState.Locked ? "已锁定" : "已解锁";
            var item = new RadioMenuFlyoutItem
            {
                Text = $"{account.Email} · {account.CompactServerLabel} · {lockLabel}",
                GroupName = "VaultAccounts",
                IsChecked = account.Active,
            };
            AutomationProperties.SetName(item, $"{account.Email} · {account.ServerLabel} · {lockLabel}");
            ToolTipService.SetToolTip(item, $"{account.Email}\n{account.ServerLabel} · {lockLabel}");
            item.Click += async (_, _) =>
            {
                if (!account.Active) await RunAccountCommandAsync(() => RustCore.SwitchAccount(account.Id));
            };
            menu.Items.Add(item);
        }
        menu.Items.Add(new MenuFlyoutSeparator());
        var add = new MenuFlyoutItem { Text = "添加账户", Icon = new SymbolIcon(Symbol.Add), IsEnabled = _accounts.Length < 5 };
        add.Click += async (_, _) => await ShowSignInDialogAsync();
        menu.Items.Add(add);
        if (ActiveAccount is { State: VaultSessionState.Unlocked, NeedsSignIn: true } accountToReconnect)
        {
            var reconnect = new MenuFlyoutItem { Text = "重新连接当前账户" };
            reconnect.Click += async (_, _) => await ShowSignInDialogAsync(accountToReconnect);
            menu.Items.Add(reconnect);
        }
        var logout = new MenuFlyoutItem { Text = "退出当前账户" };
        logout.Click += async (_, _) => await RunAccountCommandAsync(() => { RustCore.Logout(); return RustCore.GetAccounts(); });
        menu.Items.Add(logout);
        menu.ShowAt(SignInButton);
    }

    private async Task RunAccountCommandAsync(Func<VaultAccount[]> change)
    {
        try { await ChangeAccountsAsync(change); }
        catch (VaultException error)
        {
            if (_closed) return;
            await ShowAccountErrorAsync(error);
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }

    private async Task ShowAccountErrorAsync(VaultException error)
    {
        if (_closed || NavView.XamlRoot is not { } root) return;
        await new ContentDialog
        {
            XamlRoot = root,
            Title = "账户操作未完成",
            Content = error.UserMessage,
            CloseButtonText = "关闭",
        }.ShowAsync();
    }

    private async Task ShowUnlockDialogAsync()
    {
        if (_closed || _signInDialogOpen || _accountTransition) return;
        if (ActiveAccount is { NeedsSignIn: true } restored)
        {
            await ShowSignInDialogAsync(restored);
            return;
        }
        _signInDialogOpen = true;
        UpdateSessionControls(VaultSessionState.Locked);

        var helloAccount = ActiveAccount is { HelloEnabled: true } account ? account.Id : null;
        var pinAccount = ActiveAccount is { PinEnabled: true } pAccount ? pAccount.Id : null;
        var usePin = pinAccount is not null;
        var password = new PasswordBox { Header = usePin ? "PIN 码" : "主密码", MinWidth = 300 };
        var switchModeButton = new HyperlinkButton
        {
            Content = usePin ? "改用主密码" : "改用 PIN 码",
            Visibility = pinAccount is not null ? Visibility.Visible : Visibility.Collapsed,
            Padding = new Thickness(0),
        };
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap, MaxWidth = 360 };
        var panel = new StackPanel { Spacing = 8 };
        panel.Children.Add(PasswordVisibility.WithToggle(password));
        panel.Children.Add(switchModeButton);
        panel.Children.Add(status);
        var dialog = new ContentDialog
        {
            XamlRoot = NavView.XamlRoot,
            Title = "解锁保险库",
            Content = panel,
            PrimaryButtonText = "解锁",
            SecondaryButtonText = helloAccount is null ? string.Empty : "Windows Hello",
            CloseButtonText = "取消",
            DefaultButton = helloAccount is null ? ContentDialogButton.Primary : ContentDialogButton.Secondary,
            IsPrimaryButtonEnabled = false,
        };

        switchModeButton.Click += (_, _) =>
        {
            if (_unlockRequest is not null || pinAccount is null) return;
            usePin = !usePin;
            password.Password = string.Empty;
            password.Header = usePin ? "PIN 码" : "主密码";
            switchModeButton.Content = usePin ? "改用主密码" : "改用 PIN 码";
            status.Text = string.Empty;
            password.Focus(FocusState.Programmatic);
        };
        password.PasswordChanged += (_, _) =>
        {
            dialog.IsPrimaryButtonEnabled = _unlockRequest is null && password.Password.Length != 0;
            dialog.DefaultButton = password.Password.Length != 0 || helloAccount is null
                ? ContentDialogButton.Primary : ContentDialogButton.Secondary;
        };
        dialog.Opened += async (_, _) =>
        {
            if (helloAccount is not null && DesktopPreferences.Read(DesktopPreferences.AutoPromptHello, fallback: false))
                await UnlockAsync(true);
            else if (helloAccount is null || usePin)
                password.Focus(FocusState.Programmatic);
        };
        dialog.Closing += (_, args) =>
        {
            if (_unlockRequest is null) return;
            args.Cancel = true;
            _unlockRequest.Cancel();
        };
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            await UnlockAsync(false);
        };
        dialog.SecondaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            await UnlockAsync(true);
        };
        async Task UnlockAsync(bool hello)
        {
            if (_unlockRequest is not null || (hello ? helloAccount is null : password.Password.Length == 0)) return;
            var entered = hello ? string.Empty : password.Password;
            var usingPin = !hello && usePin && pinAccount is not null;
            using var request = _unlockRequest = new CancellationTokenSource();
            password.Password = string.Empty;
            password.IsEnabled = switchModeButton.IsEnabled = dialog.IsPrimaryButtonEnabled = dialog.IsSecondaryButtonEnabled = false;
            dialog.CloseButtonText = "取消解锁";
            status.Text = hello
                ? "请按照 Windows Hello 提示验证…"
                : usingPin ? "正在验证 PIN 码…" : "正在本机解锁…";
            var unlocked = false;
            try
            {
                if (usingPin)
                    await Task.Run(() => RustCore.UnlockPin(pinAccount!, entered, request.Token));
                else
                    await UnlockLocalVaultAsync(hello ? helloAccount : null, entered, request.Token);
                unlocked = true;
            }
            catch (OperationCanceledException) { status.Text = "解锁已取消。"; }
            catch (VaultException error)
            {
                if (usingPin && error.Code is "pinRevoked" or "pinNotEnabled")
                {
                    pinAccount = null;
                    usePin = false;
                    switchModeButton.Visibility = Visibility.Collapsed;
                    password.Header = "主密码";
                }
                status.Text = error.UserMessage;
            }
            catch (Exception error) when (IsCoreUnavailable(error))
            { status.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally
            {
                entered = string.Empty;
                _unlockRequest = null;
                if (!_closed)
                {
                    if (unlocked || request.IsCancellationRequested) dialog.Hide();
                    else
                    {
                        password.IsEnabled = true;
                        switchModeButton.IsEnabled = pinAccount is not null;
                        dialog.IsSecondaryButtonEnabled = helloAccount is not null;
                        dialog.CloseButtonText = "取消";
                        password.Focus(FocusState.Programmatic);
                    }
                }
            }
        }

        try { await dialog.ShowAsync(); }
        finally
        {
            password.Password = string.Empty;
            try
            {
                if (!_closed)
                {
                    var accounts = await Task.Run(RustCore.GetAccounts);
                    if (!_closed) ApplyAccountSnapshot(accounts);
                }
            }
            finally { _signInDialogOpen = false; }
            if (!_closed)
            {
                var state = RustCore.SessionState;
                UpdateSessionControls(state);
                if (!await ApplyPendingWindowsTimeoutAsync() && state == VaultSessionState.Unlocked)
                    await ShowActiveVaultAsync(state, forceSync: false);
            }
        }
    }
}
