using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using System.Net;
using System.Threading.Tasks;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Bitwarden_WinUI.Controls;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow : Window
{
    private string? _folderToOpen;

    public MainWindow()
    {
        InitializeComponent();
        ClipboardCleaner.Initialize(WinRT.Interop.WindowNative.GetWindowHandle(this));
        ApplyDesktopTheme(DesktopPreferences.ReadTheme());
        ClipboardCleaner.Copied += MinimizeAfterCopy;
        try
        {
            if (!RustCore.ConfigureLocalCache(Windows.Storage.ApplicationData.Current.LocalFolder.Path))
                WarnAboutAccountStorage("本机缓存不可用", "本次会话仍可在线使用，但关闭应用后可能无法恢复保险库。");
        }
        catch (Exception error) when (IsCoreUnavailable(error) || error is System.Runtime.InteropServices.COMException)
        {
            WarnAboutAccountStorage("本机缓存不可用", "请检查应用的安装和 Windows 本机数据目录。");
        }

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Standard;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        AccountSettingsScope.SignedOut += GeneratorPage.ResetSavedAlgorithm;
        InitializeVaultTimeout();
        InitializeTray();
        try
        {
            ApplyAccountSnapshot(ApplyRestartTimeouts(RustCore.GetAccounts()));
            UpdateSessionControls(RustCore.SessionState);
        }
        catch (VaultException error) when (error.Code == "cacheRevocationFailed")
        {
            ApplyAccountSnapshot(RustCore.GetAccounts());
            UpdateSessionControls(RustCore.SessionState);
            WarnAboutAccountStorage("重启时自动登出未完成", error.UserMessage);
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
        NavFrame.Navigated += NavFrame_Navigated;
        NavFrame.Navigate(typeof(HomePage), "vault");
        StartPasskeyProvider();
        Activated += (_, args) =>
        {
            if (args.WindowActivationState != WindowActivationState.Deactivated && NavFrame.Content is SettingsPage settings)
            {
                settings.RefreshPasskeyStatus();
                _ = settings.RefreshStartupAsync();
            }
        };
        Closed += (_, _) =>
        {
            _closed = true;
            _searchDispatch?.Cancel();
            UnbindSearchInput();
            DisposeTray();
            PrepareForExit();
            ClipboardCleaner.Stop();
        };
    }

    private void WarnAboutAccountStorage(string title, string message)
    {
        AccountWarning.Title = title;
        AccountWarning.Message = message;
        AccountWarning.IsOpen = true;
    }

    private void TitleBar_PaneToggleRequested(TitleBar sender, object args)
    {
        NavView.IsPaneOpen = !NavView.IsPaneOpen;
    }

    private void NavView_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.IsSettingsSelected)
        {
            NavFrame.Navigate(typeof(SettingsPage));
            return;
        }

        if (args.SelectedItem is not NavigationViewItem item || item.Tag is not string tag)
        {
            return;
        }

        if (tag == "generator")
        {
            NavFrame.Navigate(typeof(GeneratorPage));
        }
        else if (tag == "folder-manager")
        {
            NavFrame.Navigate(typeof(FolderManagementPage));
        }
        else if (tag == "sends")
        {
            NavFrame.Navigate(typeof(SendsPage));
        }
        else if (tag == "about")
        {
            NavFrame.Navigate(typeof(AboutPage));
        }
        else
        {
            var folder = _folderToOpen;
            _folderToOpen = null;
            if (NavFrame.Content is HomePage existingHome)
            {
                existingHome.SwitchCategory(tag, folder);
            }
            else
            {
                NavFrame.Navigate(typeof(HomePage),
                    folder is null ? tag : new HomePage.FolderNavigation(folder));
            }
        }
    }

    private void OpenVaultFolder(object? sender, string folderId)
    {
        SearchBox.Text = string.Empty;
        _folderToOpen = folderId;
        NavView.SelectedItem = VaultNavigationItem;
    }

    private void OpenFolderManager(object? sender, EventArgs e)
    {
        SearchBox.Text = string.Empty;
        NavView.SelectedItem = FolderManagementNavigationItem;
    }

    private void SearchBox_QuerySubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        _searchDispatch?.Cancel();
        if (NavFrame.Content is HomePage page)
        {
            page.SetSearch(args.QueryText);
        }
        else if (NavFrame.Content is FolderManagementPage folderPage)
        {
            folderPage.SetSearch(args.QueryText);
        }
        else if (NavFrame.Content is SendsPage sends) sends.SetSearch(args.QueryText);
        else
        {
            NavFrame.Navigate(typeof(HomePage), "vault");
            if (NavFrame.Content is HomePage homePage)
            {
                homePage.SetSearch(args.QueryText);
            }
        }
    }

    private void SearchBox_TextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args) =>
        RequestSearch(sender.Text);

    private async void Sync_Click(object sender, RoutedEventArgs e)
    {
        if (ActiveAccount is { State: VaultSessionState.Unlocked, NeedsSignIn: true } offline)
        {
            await ShowSignInDialogAsync(offline);
            return;
        }
        if (ActiveAccount is { State: VaultSessionState.Unlocked } account && _reconnectSyncErrors.ContainsKey(account.Id))
        {
            await RetryReconnectSyncAsync(account);
            return;
        }
        if (NavFrame.Content is HomePage page) await page.RefreshAsync(true);
        else if (NavFrame.Content is FolderManagementPage folderPage) await folderPage.RefreshAsync(true);
        else if (NavFrame.Content is SendsPage sends) await sends.RefreshAsync();
        else
        {
            NavFrame.Navigate(typeof(HomePage), "vault");
            if (NavFrame.Content is HomePage homePage) await homePage.RefreshAsync(true);
        }
    }

    private void NavFrame_Navigated(object sender, NavigationEventArgs e)
    {
        _searchDispatch?.Cancel();
        if (NavFrame.BackStack.Count > 0) NavFrame.BackStack.Clear();
        if (NavFrame.ForwardStack.Count > 0) NavFrame.ForwardStack.Clear();
        SearchBox.PlaceholderText = e.Content switch
        {
            FolderManagementPage => "搜索文件夹",
            SendsPage => "搜索 Send",
            _ => "搜索保险库",
        };
        if (e.Content is HomePage page)
        {
            page.SignInRequested -= HomePage_SignInRequested;
            page.SignInRequested += HomePage_SignInRequested;
            page.ManageFoldersRequested -= OpenFolderManager;
            page.ManageFoldersRequested += OpenFolderManager;
            page.SetSearch(SearchBox.Text);
        }
        else if (e.Content is SettingsPage settings)
        {
            settings.VaultTimeoutChanged = value => SaveVaultTimeout(settings, value);
            settings.ApplyTheme = ApplyDesktopTheme;
            settings.ChangeHelloUnlock = () => ChangeHelloUnlockAsync(settings);
            settings.ExportVault = () => ExportVaultAsync(settings);
            settings.ImportVault = () => ImportVaultAsync(settings);
            settings.ShowHelloUnlock(ActiveAccount, _signInDialogOpen || _accountTransition);
            settings.ShowVaultExport(ActiveAccount, _signInDialogOpen || _accountTransition);
            UpdateTimeoutControls(settings);
        }
        else if (e.Content is SendsPage sends)
        {
            sends.ConnectRequested -= SendsPage_ConnectRequested;
            sends.ConnectRequested += SendsPage_ConnectRequested;
            _ = sends.ShowAccountAsync(ActiveAccount, SearchBox.Text);
        }
        else if (e.Content is FolderManagementPage folders)
        {
            folders.SignInRequested -= HomePage_SignInRequested;
            folders.SignInRequested += HomePage_SignInRequested;
            folders.OpenFolderRequested -= OpenVaultFolder;
            folders.OpenFolderRequested += OpenVaultFolder;
            folders.SetSearch(SearchBox.Text);
        }
    }

    private async void SignIn_Click(object sender, RoutedEventArgs e)
    {
        if (_closed || _accountTransition || _signInDialogOpen) return;
        try
        {
            if (ActiveAccount is null)
            {
                await ShowSignInDialogAsync();
                return;
            }
            ShowAccountMenu();
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }

    private async void SendsPage_ConnectRequested(object? sender, EventArgs e)
    {
        if (ActiveAccount is { State: VaultSessionState.Unlocked, NeedsSignIn: true } account)
        {
            try { await ShowSignInDialogAsync(account); }
            catch (Exception error) when (IsCoreUnavailable(error)) { }
        }
        else HomePage_SignInRequested(sender, e);
    }

    private async void HomePage_SignInRequested(object? sender, EventArgs e)
    {
        try
        {
            if (RustCore.SessionState == VaultSessionState.Locked) await ShowUnlockDialogAsync();
            else await ShowSignInDialogAsync();
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }

    private bool _signInDialogOpen;

    private async Task ShowSignInDialogAsync(VaultAccount? reconnect = null)
    {
        if (_signInDialogOpen || _accountTransition)
        {
            return;
        }

        var previous = ActiveAccount;
        _signInDialogOpen = true;
        UpdateSessionControls(RustCore.SessionState);
        var changed = false;
        string? syncError = null;
        try
        {
            var result = await ShowSignInDialogCoreAsync(reconnect);
            if (_closed) return;
            RustCore.CancelPendingLogin();
            if (!result.Opened) return;
            syncError = result.SyncError;
            var accounts = await Task.Run(RustCore.GetAccounts);
            if (!_closed)
            {
                var current = accounts.FirstOrDefault(account => account.Active);
                changed = previous?.Id != current?.Id || previous?.NeedsSignIn != current?.NeedsSignIn ||
                    previous?.State != current?.State;
                if (changed && (previous?.Id != current?.Id || previous?.NeedsSignIn != current?.NeedsSignIn))
                    ClearActiveAccountContents(cancelPasskeys: previous is not null);
                ApplyAccountSnapshot(accounts);
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
        finally
        {
            _signInDialogOpen = false;
            if (!_closed)
            {
                try
                {
                    var state = RustCore.SessionState;
                    UpdateSessionControls(state);
                    if (!await ApplyPendingWindowsTimeoutAsync() && changed)
                    {
                        if (syncError is not null && ActiveAccount is { } current)
                            ReportReconnectSyncError(current.Id, syncError);
                        await ShowActiveVaultAsync(state, forceSync: reconnect is null, allowSyncFallback: syncError is null);
                    }
                }
                catch (Exception error) when (IsCoreUnavailable(error)) { SignInButton.IsEnabled = true; }
            }
        }
    }

    private async Task<(bool Opened, string? SyncError)> ShowSignInDialogCoreAsync(VaultAccount? reconnect)
    {
        var opened = false;
        string? syncError = null;
        var allowLocalUnlock = reconnect is { State: VaultSessionState.Locked };
        var allowHello = allowLocalUnlock && reconnect!.HelloEnabled;
        var environment = reconnect is null ? LoginEnvironment.Load() : new LoginEnvironment(reconnect.ServerMode, reconnect.ServerUrl);
        var serverBox = new ComboBox
        {
            Name = "ServerRegion",
            Header = "服务器区域",
            HorizontalAlignment = HorizontalAlignment.Stretch,
            Items = {
                new ComboBoxItem { Content = "Bitwarden.com（美国）", Tag = 0u },
                new ComboBoxItem { Content = "Bitwarden.eu（欧盟）", Tag = 1u },
                new ComboBoxItem { Content = "自托管 / 自定义服务器", Tag = 2u },
            },
            SelectedIndex = (int)environment.Mode,
            IsEnabled = reconnect is null,
        };
        var serverUrlBox = new TextBox
        {
            Name = "CustomServerUrl",
            Header = "服务器 URL",
            PlaceholderText = "https://vault.example.com",
            Text = environment.CustomUrl,
            IsReadOnly = reconnect is not null,
            Visibility = environment.Mode == 2 ? Visibility.Visible : Visibility.Collapsed,
        };
        var serverHelp = new TextBlock
        {
            Text = "填写服务器根地址，无需添加 /api 或 /identity。HTTP 仅支持本机回环地址。",
            TextWrapping = TextWrapping.Wrap,
            Visibility = reconnect is null ? serverUrlBox.Visibility : Visibility.Collapsed,
        };
        var emailBox = new TextBox { Header = "电子邮箱", PlaceholderText = "name@example.com",
            Text = reconnect?.Email ?? string.Empty, IsReadOnly = reconnect is not null };
        var passwordBox = new PasswordBox { Header = "主密码", PlaceholderText = "输入 Bitwarden 主密码" };
        var accessHelp = new TextBlock
        {
            Text = "输入一次主密码即可解锁并连接服务器。断网时可选择只读离线解锁。" +
                (allowHello ? " Windows Hello 仅解锁本机缓存，不会登录服务器。" : ""),
            TextWrapping = TextWrapping.Wrap,
            Visibility = allowLocalUnlock ? Visibility.Visible : Visibility.Collapsed,
        };
        var helloButton = new Button { Content = "Windows Hello", Visibility = allowHello ? Visibility.Visible : Visibility.Collapsed };
        var providerBox = new ComboBox { Header = "二步验证方式", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Stretch };
        var tokenBox = new TextBox { Header = "验证码", PlaceholderText = "输入一次性验证码", Visibility = Visibility.Collapsed };
        var sendEmailButton = new Button { Content = "发送邮箱验证码", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Left };
        var rememberTwoFactorBox = new CheckBox
        {
            Name = "RememberTwoFactor",
            Content = "记住我（30 天）",
            IsChecked = false,
            Visibility = Visibility.Collapsed,
        };
        var rememberTwoFactorHelp = new TextBlock
        {
            Text = "此设备 30 天内免二步验证，仍需输入主密码。仅在可信的个人设备上使用；服务器可提前撤销。",
            TextWrapping = TextWrapping.Wrap,
            Visibility = Visibility.Collapsed,
        };
        var deviceOtpBox = new TextBox { Header = "新设备邮箱验证码", PlaceholderText = "输入新设备验证邮件中的验证码", Visibility = Visibility.Collapsed };
        var resendDeviceOtpButton = new Button { Content = "重新发送新设备验证码", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Left };
        var progress = new ProgressBar { IsIndeterminate = true, Visibility = Visibility.Collapsed };
        var statusText = new TextBlock { Name = "LoginStatus", TextWrapping = TextWrapping.Wrap };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetLiveSetting(statusText, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        var content = new StackPanel { Spacing = 12, MinWidth = 320, MaxWidth = 440 };
        if (reconnect is null)
        {
            foreach (var element in new UIElement[] { serverBox, serverUrlBox, serverHelp, emailBox }) content.Children.Add(element);
        }
        else
        {
            content.Children.Add(new TextBlock { Text = $"{reconnect.Email} · {reconnect.ServerLabel}", TextWrapping = TextWrapping.Wrap });
        }
        foreach (var element in new UIElement[] { accessHelp, PasswordVisibility.WithToggle(passwordBox),
            helloButton, deviceOtpBox, resendDeviceOtpButton, providerBox, tokenBox, sendEmailButton,
            rememberTwoFactorBox, rememberTwoFactorHelp, progress, statusText })
        {
            content.Children.Add(element);
        }

        var dialog = new ContentDialog
        {
            Title = allowLocalUnlock ? "解锁保险库" : reconnect is null ? "登录 Bitwarden" : "重新连接当前账户",
            Content = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto },
            PrimaryButtonText = allowLocalUnlock ? "解锁并同步" : "登录",
            SecondaryButtonText = allowLocalUnlock ? "仅离线解锁" : string.Empty,
            IsPrimaryButtonEnabled = !allowLocalUnlock,
            IsSecondaryButtonEnabled = false,
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = AppTitleBar.XamlRoot,
        };

        var busy = false;
        var closingRequested = false;
        var unsupportedTwoFactor = false;
        var deviceVerificationRequired = false;
        var pendingDeviceOtp = string.Empty;
        var deviceIdentifier = LoginEnvironment.GetDeviceIdentifier();
        CancellationTokenSource? requestCancellation = null;

        (uint Mode, string Url) GetServerSelection()
        {
            var mode = serverBox.SelectedItem is ComboBoxItem item && item.Tag is uint selectedMode
                ? selectedMode : 0u;
            return (mode, mode == 2 ? serverUrlBox.Text.Trim() : string.Empty);
        }

        void SetBusy(bool value)
        {
            busy = value;
            progress.Visibility = value ? Visibility.Visible : Visibility.Collapsed;
            serverBox.IsEnabled = !value && reconnect is null;
            serverUrlBox.IsEnabled = !value;
            emailBox.IsEnabled = passwordBox.IsEnabled = !value;
            providerBox.IsEnabled = tokenBox.IsEnabled = rememberTwoFactorBox.IsEnabled = !value;
            deviceOtpBox.IsEnabled = sendEmailButton.IsEnabled = resendDeviceOtpButton.IsEnabled = !value;
            helloButton.IsEnabled = !value;
            dialog.IsPrimaryButtonEnabled = !value && !unsupportedTwoFactor && (!allowLocalUnlock || passwordBox.Password.Length != 0);
            dialog.IsSecondaryButtonEnabled = allowLocalUnlock && !value && passwordBox.Password.Length != 0;
            dialog.CloseButtonText = value ? "取消请求" : "取消";
        }

        CancellationToken BeginRequest()
        {
            _unlockRequest = requestCancellation = new CancellationTokenSource();
            SetBusy(true);
            return requestCancellation.Token;
        }

        void ResetTwoFactor()
        {
            if (busy) return;
            unsupportedTwoFactor = false;
            deviceVerificationRequired = false;
            pendingDeviceOtp = string.Empty;
            providerBox.Items.Clear();
            providerBox.Visibility = tokenBox.Visibility = sendEmailButton.Visibility = Visibility.Collapsed;
            deviceOtpBox.Visibility = resendDeviceOtpButton.Visibility = Visibility.Collapsed;
            rememberTwoFactorBox.Visibility = rememberTwoFactorHelp.Visibility = Visibility.Collapsed;
            rememberTwoFactorBox.IsChecked = false;
            deviceOtpBox.Text = string.Empty;
            tokenBox.Text = string.Empty;
            statusText.Text = string.Empty;
            SetBusy(false);
        }

        void RequestClose()
        {
            if (!busy) return;
            closingRequested = true;
            statusText.Text = "正在取消请求…";
            requestCancellation?.Cancel();
        }

        void FinishRequest()
        {
            if (requestCancellation is null) return;
            _unlockRequest = null;
            requestCancellation.Dispose();
            requestCancellation = null;
            if (_closed) return;
            SetBusy(false);
            if (closingRequested || opened) dialog.Hide();
        }

        bool Validate(string email, string password, uint serverMode, string serverUrl)
        {
            if (string.IsNullOrWhiteSpace(email) || string.IsNullOrEmpty(password))
            {
                statusText.Text = "请填写电子邮箱和主密码。";
                if (string.IsNullOrWhiteSpace(email)) emailBox.Focus(FocusState.Programmatic);
                else passwordBox.Focus(FocusState.Programmatic);
                return false;
            }
            if (!RustCore.IsValidServer(serverMode, serverUrl))
            {
                statusText.Text = "服务器地址无效。请填写完整的 HTTPS 根地址，例如 https://vault.example.com。";
                serverUrlBox.Focus(FocusState.Programmatic);
                return false;
            }
            if (reconnect is null) new LoginEnvironment(serverMode, serverUrlBox.Text.Trim()).Save();
            return true;
        }

        providerBox.SelectionChanged += (_, _) =>
        {
            var isEmail = providerBox.SelectedItem is ComboBoxItem item && item.Tag is uint provider && provider == 1;
            sendEmailButton.Visibility = isEmail ? Visibility.Visible : Visibility.Collapsed;
            tokenBox.Text = string.Empty;
        };
        serverBox.SelectionChanged += (_, _) =>
        {
            var visibility = GetServerSelection().Mode == 2 ? Visibility.Visible : Visibility.Collapsed;
            serverUrlBox.Visibility = serverHelp.Visibility = visibility;
            ResetTwoFactor();
        };
        serverUrlBox.TextChanged += (_, _) => ResetTwoFactor();
        emailBox.TextChanged += (_, _) => ResetTwoFactor();
        passwordBox.PasswordChanged += (_, _) => ResetTwoFactor();
        dialog.CloseButtonClick += (_, args) =>
        {
            if (busy) { args.Cancel = true; RequestClose(); }
        };
        dialog.Closing += (_, args) =>
        {
            if (busy) { args.Cancel = true; RequestClose(); }
        };

        if (reconnect is not null) dialog.Opened += (_, _) => passwordBox.Focus(FocusState.Programmatic);

        async Task TryUnlockLocalAsync(bool hello)
        {
            if (busy || !allowLocalUnlock || (hello ? !allowHello : passwordBox.Password.Length == 0)) return;
            var password = hello ? string.Empty : passwordBox.Password;
            var token = BeginRequest();
            passwordBox.Password = string.Empty;
            statusText.Text = hello ? "请按照 Windows Hello 提示验证…" : "正在本机解锁…";
            try
            {
                await UnlockLocalVaultAsync(hello ? reconnect!.Id : null, password, token);
                opened = true;
            }
            catch (OperationCanceledException) { statusText.Text = "解锁已取消。"; }
            catch (VaultException error) { statusText.Text = error.UserMessage; }
            catch (Exception error) when (IsCoreUnavailable(error)) { statusText.Text = "无法加载 Rust 核心，请修复应用安装。"; }
            finally
            {
                FinishRequest();
                if (!_closed && !opened && !closingRequested) passwordBox.Focus(FocusState.Programmatic);
            }
        }

        dialog.SecondaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            await TryUnlockLocalAsync(hello: false);
        };
        helloButton.Click += async (_, _) => await TryUnlockLocalAsync(hello: true);

        sendEmailButton.Click += async (_, _) =>
        {
            if (busy) return;
            try
            {
                var email = emailBox.Text.Trim();
                var password = passwordBox.Password;
                var (mode, url) = GetServerSelection();
                if (!Validate(email, password, mode, url)) return;
                var cancellationToken = BeginRequest();
                statusText.Text = "正在请求发送验证码…";
                var result = await Task.Run(() => RustCore.SendTwoFactorEmail(email, password, mode, url, cancellationToken));
                statusText.Text = result switch
                {
                    AuthStatus.Authenticated => "验证码发送请求已成功。请检查邮箱。",
                    AuthStatus.TimedOut => "服务器响应超时，请检查服务器地址和网络连接后重试。",
                    AuthStatus.Cancelled => "请求已取消。",
                    _ => "无法发送验证码。请检查账户信息与网络连接后重试。",
                };
            }
            catch (OperationCanceledException) { statusText.Text = "请求已取消。"; }
            catch (Exception error) when (IsCoreUnavailable(error)) { statusText.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally { FinishRequest(); }
        };

        resendDeviceOtpButton.Click += async (_, _) =>
        {
            if (busy) return;
            try
            {
                var email = emailBox.Text.Trim();
                var password = passwordBox.Password;
                var (mode, url) = GetServerSelection();
                if (!Validate(email, password, mode, url)) return;
                var cancellationToken = BeginRequest();
                statusText.Text = "正在重新发送新设备验证码…";
                var result = await Task.Run(() => RustCore.ResendNewDeviceVerification(
                    email, password, deviceIdentifier, mode, url, cancellationToken));
                statusText.Text = result switch
                {
                    AuthStatus.Authenticated => "已请求重新发送验证码。请检查账户邮箱。",
                    AuthStatus.TimedOut => "服务器响应超时，请检查网络后重试。",
                    AuthStatus.Cancelled => "请求已取消。",
                    _ => "无法重新发送验证码。请确认登录信息后重试。",
                };
            }
            catch (OperationCanceledException) { statusText.Text = "请求已取消。"; }
            catch (Exception error) when (IsCoreUnavailable(error)) { statusText.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally { FinishRequest(); }
        };

        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (busy) return;
            try
            {
                var email = emailBox.Text.Trim();
                var password = passwordBox.Password;
                var (mode, url) = GetServerSelection();
                if (!Validate(email, password, mode, url)) return;
                var needsToken = tokenBox.Visibility == Visibility.Visible;
                var token = tokenBox.Text.Trim();
                if (needsToken && token.Length == 0)
                {
                    statusText.Text = "请输入二步验证码。";
                    tokenBox.Focus(FocusState.Programmatic);
                    return;
                }
                var deviceOtp = deviceVerificationRequired ? deviceOtpBox.Text.Trim() : pendingDeviceOtp;
                if (deviceVerificationRequired && deviceOtp.Length == 0)
                {
                    statusText.Text = "请输入新设备验证邮件中的验证码。";
                    deviceOtpBox.Focus(FocusState.Programmatic);
                    return;
                }
                var provider = providerBox.SelectedItem is ComboBoxItem item && item.Tag is uint id ? id : 0;
                var rememberTwoFactor = needsToken && rememberTwoFactorBox.IsChecked == true;
                var cancellationToken = BeginRequest();
                statusText.Text = "正在验证账户…";
                var result = await Task.Run(() => RustCore.Login(email, password, provider,
                    needsToken ? token : string.Empty, deviceOtp, mode, url, deviceIdentifier, cancellationToken, rememberTwoFactor));
                if (result.Status == AuthStatus.Authenticated)
                {
                    opened = true;
                    passwordBox.Password = string.Empty;
                    password = string.Empty;
                    deviceVerificationRequired = false;
                    pendingDeviceOtp = string.Empty;
                    if (reconnect is not null && !_closed)
                    {
                        statusText.Text = "正在同步保险库…";
                        try { await Task.Run(() => RustCore.SyncVault(cancellationToken)); }
                        catch (VaultException error) { syncError = error.UserMessage; }
                        catch (OperationCanceledException) { syncError = "同步已取消。"; }
                    }
                    return;
                }
                if (result.Status is AuthStatus.NewDeviceVerificationRequired or AuthStatus.InvalidNewDeviceVerificationCode)
                {
                    deviceVerificationRequired = true;
                    deviceOtpBox.Visibility = resendDeviceOtpButton.Visibility = Visibility.Visible;
                    providerBox.Visibility = tokenBox.Visibility = sendEmailButton.Visibility = Visibility.Collapsed;
                    rememberTwoFactorBox.Visibility = rememberTwoFactorHelp.Visibility = Visibility.Collapsed;
                    tokenBox.Text = string.Empty;
                    statusText.Text = result.Status == AuthStatus.InvalidNewDeviceVerificationCode
                        ? "新设备验证码无效或已过期。请重新输入或重新发送。"
                        : "此账户要求验证新设备。服务器已发送验证码，请检查账户邮箱后输入。";
                    if (result.Status == AuthStatus.NewDeviceVerificationRequired) deviceOtpBox.Text = string.Empty;
                    return;
                }
                if (result.Status == AuthStatus.TwoFactorRequired)
                {
                    if (deviceVerificationRequired)
                    {
                        pendingDeviceOtp = deviceOtp;
                        deviceVerificationRequired = false;
                        deviceOtpBox.Visibility = resendDeviceOtpButton.Visibility = Visibility.Collapsed;
                    }
                    var previousProvider = providerBox.SelectedItem is ComboBoxItem previous ? previous.Tag : null;
                    var advertised = result.AvailableTwoFactorMethods;
                    var manualAuthenticator = TwoFactorMethods.OffersManualAuthenticator(advertised, mode);
                    PopulateTwoFactorMethods(providerBox, advertised, manualAuthenticator);
                    foreach (var candidate in providerBox.Items.OfType<ComboBoxItem>())
                    {
                        if (Equals(candidate.Tag, previousProvider)) providerBox.SelectedItem = candidate;
                    }
                    unsupportedTwoFactor = providerBox.Items.Count == 0;
                    providerBox.Visibility = tokenBox.Visibility = unsupportedTwoFactor ? Visibility.Collapsed : Visibility.Visible;
                    rememberTwoFactorBox.Visibility = rememberTwoFactorHelp.Visibility = tokenBox.Visibility;
                    tokenBox.Text = string.Empty;
                    sendEmailButton.Visibility = providerBox.SelectedItem is ComboBoxItem { Tag: TwoFactorMethods.Email }
                        ? Visibility.Visible : Visibility.Collapsed;
                    statusText.Text = manualAuthenticator
                        ? TwoFactorMethods.UnsupportedReason(advertised) + (needsToken
                            ? "服务器没有接受此次验证。如果确定同一账户启用了身份验证器，可用下一组 6 位验证码重试；否则请用网页保险库或官方客户端。"
                            : "如果你确实为此账户启用了 TOTP，可手动输入 6 位验证码；是否接受仍由服务器决定。")
                        : unsupportedTwoFactor
                            ? TwoFactorMethods.UnsupportedReason(advertised) + "请使用网页保险库或官方客户端完成验证。"
                            : providerBox.SelectedItem is ComboBoxItem { Tag: TwoFactorMethods.Authenticator }
                                ? "请输入身份验证器为该账户生成的 6 位验证码。"
                                : "请选择二步验证方式并输入新的验证码。";
                    return;
                }
                if (result.Status == AuthStatus.InvalidTwoFactorCode)
                {
                    tokenBox.Text = string.Empty;
                    statusText.Text = provider == 0
                        ? "身份验证器验证码无效、过期或已使用。请确认设备时间同步，并等待下一组 6 位验证码后重试。"
                        : "二步验证码无效或已过期。请用新的验证码重试。";
                    return;
                }
                statusText.Text = result.Status switch
                {
                    AuthStatus.TimedOut => "服务器响应超时，请检查服务器地址和网络连接后重试。",
                    AuthStatus.Cancelled => "请求已取消。",
                    AuthStatus.RateLimited => "服务器暂时限制登录尝试。请等待一段时间，不要连续重试。",
                    AuthStatus.AccountAlreadySignedIn => "该账户已登录，请从账户菜单切换。",
                    AuthStatus.AccountLimitReached => "最多可同时登录 5 个账户，请先退出其中一个。",
                    AuthStatus.Failed when deviceVerificationRequired && !needsToken => "新设备验证码无效或已过期。请重新输入或重新发送。",
                    AuthStatus.Failed when allowLocalUnlock => "未能连接账户。请检查主密码或网络；也可选择仅离线解锁。",
                    _ => "登录失败。请检查账户、主密码、验证码和服务器地址后重试。",
                };
            }
            catch (OperationCanceledException) { statusText.Text = "请求已取消。"; }
            catch (Exception error) when (IsCoreUnavailable(error)) { statusText.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally
            {
                FinishRequest();
                if (!_closed && !opened && !closingRequested && tokenBox.Visibility == Visibility.Visible && !unsupportedTwoFactor)
                    tokenBox.Focus(FocusState.Programmatic);
            }
        };

        try
        {
            await dialog.ShowAsync();
        }
        finally
        {
            if (requestCancellation is not null)
            {
                _unlockRequest = null;
                requestCancellation.Cancel();
                requestCancellation.Dispose();
            }
            passwordBox.Password = string.Empty;
            deviceOtpBox.Text = string.Empty;
            pendingDeviceOtp = string.Empty;
            tokenBox.Text = string.Empty;
            // Server preferences are not account credentials and can be saved without logging in.
            var (mode, url) = GetServerSelection();
            try
            {
                if (reconnect is null && (mode != 2 || RustCore.IsValidServer(mode, url)))
                    new LoginEnvironment(mode, serverUrlBox.Text.Trim()).Save();
            }
            catch (Exception error) when (IsCoreUnavailable(error)) { }
        }
        return (opened, syncError);
    }

    private static bool IsCoreUnavailable(Exception error) =>
        error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;

    private static void PopulateTwoFactorMethods(ComboBox providerBox, uint advertised, bool manualAuthenticator)
    {
        providerBox.Items.Clear();
        if (manualAuthenticator || TwoFactorMethods.IsAdvertised(advertised, TwoFactorMethods.Authenticator))
        {
            providerBox.Items.Add(new ComboBoxItem { Content = manualAuthenticator ? "身份验证器（手动输入）" : "身份验证器", Tag = TwoFactorMethods.Authenticator });
        }
        if (TwoFactorMethods.IsAdvertised(advertised, TwoFactorMethods.Email))
        {
            providerBox.Items.Add(new ComboBoxItem { Content = "邮箱验证码", Tag = TwoFactorMethods.Email });
        }
        if (TwoFactorMethods.IsAdvertised(advertised, TwoFactorMethods.YubiKey))
        {
            providerBox.Items.Add(new ComboBoxItem { Content = "YubiKey", Tag = TwoFactorMethods.YubiKey });
        }
        if (providerBox.Items.Count > 0)
        {
            providerBox.SelectedIndex = 0;
        }
    }
}
