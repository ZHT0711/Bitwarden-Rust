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
    public MainWindow()
    {
        InitializeComponent();

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        NavFrame.Navigated += NavFrame_Navigated;
        NavFrame.Navigate(typeof(HomePage), "vault");
        Closed += (_, _) =>
        {
            _closed = true;
            _unlockRequest?.Cancel();
            if (NavFrame.Content is HomePage page) page.ClearSensitiveState();
            if (NavFrame.Content is GeneratorPage generator) generator.ClearSensitiveState();
            WebsiteIcons.ClearCache();
            ClipboardCleaner.ClearOwned();
            try { RustCore.Logout(); } catch (Exception error) when (IsCoreUnavailable(error)) { }
        };
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
        else if (tag == "about")
        {
            NavFrame.Navigate(typeof(AboutPage));
        }
        else
        {
            NavFrame.Navigate(typeof(HomePage), tag);
        }
    }

    private void SearchBox_QuerySubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        if (NavFrame.Content is HomePage page)
        {
            page.SetSearch(args.QueryText);
        }
        else
        {
            NavFrame.Navigate(typeof(HomePage), "vault");
            if (NavFrame.Content is HomePage homePage)
            {
                homePage.SetSearch(args.QueryText);
            }
        }
    }

    private void SearchBox_TextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
    {
        if (NavFrame.Content is HomePage page) page.SetSearch(sender.Text);
    }

    private async void Sync_Click(object sender, RoutedEventArgs e)
    {
        if (NavFrame.Content is not HomePage) NavFrame.Navigate(typeof(HomePage), "vault");
        if (NavFrame.Content is HomePage page) await page.RefreshAsync(true);
    }

    private void NavFrame_Navigated(object sender, NavigationEventArgs e)
    {
        if (e.Content is HomePage page)
        {
            page.SignInRequested -= HomePage_SignInRequested;
            page.SignInRequested += HomePage_SignInRequested;
            page.SetSearch(SearchBox.Text);
        }
    }

    private async void SignIn_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            if (RustCore.SessionState != VaultSessionState.SignedOut)
            {
                await ChangeSessionAsync(RustCore.Logout);
                return;
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            return;
        }

        await ShowSignInDialogAsync();
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

    private async Task ShowSignInDialogAsync()
    {
        if (_signInDialogOpen || _accountTransition)
        {
            return;
        }

        _signInDialogOpen = true;
        SignInButton.IsEnabled = false;
        try
        {
            await ShowSignInDialogCoreAsync();
        }
        finally
        {
            _signInDialogOpen = false;
            if (!_closed)
            {
                try { UpdateSessionControls(RustCore.SessionState); }
                catch (Exception error) when (IsCoreUnavailable(error)) { SignInButton.IsEnabled = true; }
            }
        }
    }

    private async Task ShowSignInDialogCoreAsync()
    {
        var environment = LoginEnvironment.Load();
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
        };
        var serverUrlBox = new TextBox
        {
            Name = "CustomServerUrl",
            Header = "服务器 URL",
            PlaceholderText = "https://vault.example.com",
            Text = environment.CustomUrl,
            Visibility = environment.Mode == 2 ? Visibility.Visible : Visibility.Collapsed,
        };
        var serverHelp = new TextBlock
        {
            Text = "填写服务器根地址，无需添加 /api 或 /identity。HTTP 仅支持本机回环地址。",
            TextWrapping = TextWrapping.Wrap,
            Visibility = serverUrlBox.Visibility,
        };
        var emailBox = new TextBox { Header = "电子邮箱", PlaceholderText = "name@example.com" };
        var passwordBox = new PasswordBox { Header = "主密码", PlaceholderText = "输入 Bitwarden 主密码" };
        var providerBox = new ComboBox { Header = "二步验证方式", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Stretch };
        var tokenBox = new TextBox { Header = "验证码", PlaceholderText = "输入一次性验证码", Visibility = Visibility.Collapsed };
        var sendEmailButton = new Button { Content = "发送邮箱验证码", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Left };
        var deviceOtpBox = new TextBox { Header = "新设备邮箱验证码", PlaceholderText = "输入新设备验证邮件中的验证码", Visibility = Visibility.Collapsed };
        var resendDeviceOtpButton = new Button { Content = "重新发送新设备验证码", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Left };
        var progress = new ProgressBar { IsIndeterminate = true, Visibility = Visibility.Collapsed };
        var statusText = new TextBlock { Name = "LoginStatus", TextWrapping = TextWrapping.Wrap };
        var content = new StackPanel { Spacing = 12, MinWidth = 320, MaxWidth = 440 };
        foreach (var element in new UIElement[] { serverBox, serverUrlBox, serverHelp, emailBox,
            PasswordVisibility.WithToggle(passwordBox), deviceOtpBox, resendDeviceOtpButton, providerBox, tokenBox, sendEmailButton, progress, statusText })
        {
            content.Children.Add(element);
        }

        var dialog = new ContentDialog
        {
            Title = "登录 Bitwarden",
            Content = new ScrollViewer { Content = content, VerticalScrollBarVisibility = ScrollBarVisibility.Auto },
            PrimaryButtonText = "登录",
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
            serverBox.IsEnabled = serverUrlBox.IsEnabled = !value;
            emailBox.IsEnabled = passwordBox.IsEnabled = !value;
            providerBox.IsEnabled = tokenBox.IsEnabled = !value;
            deviceOtpBox.IsEnabled = sendEmailButton.IsEnabled = resendDeviceOtpButton.IsEnabled = !value;
            dialog.IsPrimaryButtonEnabled = !value && !unsupportedTwoFactor;
            dialog.CloseButtonText = value ? "取消请求" : "取消";
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
            deviceOtpBox.Text = string.Empty;
            tokenBox.Text = string.Empty;
            statusText.Text = string.Empty;
            dialog.IsPrimaryButtonEnabled = true;
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
            requestCancellation?.Dispose();
            requestCancellation = null;
            SetBusy(false);
            if (closingRequested) dialog.Hide();
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
            new LoginEnvironment(serverMode, serverUrlBox.Text.Trim()).Save();
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

        sendEmailButton.Click += async (_, _) =>
        {
            if (busy) return;
            try
            {
                var email = emailBox.Text.Trim();
                var password = passwordBox.Password;
                var (mode, url) = GetServerSelection();
                if (!Validate(email, password, mode, url)) return;
                requestCancellation = new CancellationTokenSource();
                var cancellationToken = requestCancellation.Token;
                SetBusy(true);
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
                requestCancellation = new CancellationTokenSource();
                var cancellationToken = requestCancellation.Token;
                SetBusy(true);
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
            var authenticated = false;
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
                requestCancellation = new CancellationTokenSource();
                var cancellationToken = requestCancellation.Token;
                SetBusy(true);
                statusText.Text = "正在验证账户…";
                var result = await Task.Run(() => RustCore.Login(email, password, provider,
                    needsToken ? token : string.Empty, deviceOtp, mode, url, deviceIdentifier, cancellationToken));
                if (result.Status == AuthStatus.Authenticated)
                {
                    authenticated = true;
                    deviceVerificationRequired = false;
                    pendingDeviceOtp = string.Empty;
                    if (_closed) return;
                    WebsiteIcons.SetServer(mode, url);
                    UpdateSessionControls(VaultSessionState.Unlocked);
                    if (NavFrame.Content is HomePage page) page.ShowAuthenticated();
                    return;
                }
                if (result.Status is AuthStatus.NewDeviceVerificationRequired or AuthStatus.InvalidNewDeviceVerificationCode)
                {
                    deviceVerificationRequired = true;
                    deviceOtpBox.Visibility = resendDeviceOtpButton.Visibility = Visibility.Visible;
                    providerBox.Visibility = tokenBox.Visibility = sendEmailButton.Visibility = Visibility.Collapsed;
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
                    PopulateTwoFactorMethods(providerBox, result.AvailableTwoFactorMethods);
                    foreach (var candidate in providerBox.Items.OfType<ComboBoxItem>())
                    {
                        if (Equals(candidate.Tag, previousProvider)) providerBox.SelectedItem = candidate;
                    }
                    providerBox.Visibility = Visibility.Visible;
                    tokenBox.Visibility = Visibility.Visible;
                    tokenBox.Text = string.Empty;
                    sendEmailButton.Visibility = providerBox.SelectedItem is ComboBoxItem selected && selected.Tag is uint selectedProvider && selectedProvider == 1
                        ? Visibility.Visible : Visibility.Collapsed;
                    unsupportedTwoFactor = providerBox.Items.Count == 0;
                    statusText.Text = unsupportedTwoFactor
                        ? "此账户的二步验证方式暂不受支持。当前支持身份验证器、邮箱验证码和 YubiKey。"
                        : "请选择二步验证方式并输入新的验证码。";
                    return;
                }
                statusText.Text = result.Status switch
                {
                    AuthStatus.TimedOut => "服务器响应超时，请检查服务器地址和网络连接后重试。",
                    AuthStatus.Cancelled => "请求已取消。",
                    AuthStatus.Failed when deviceVerificationRequired && !needsToken => "新设备验证码无效或已过期。请重新输入或重新发送。",
                    _ => "登录失败。请检查账户、主密码、验证码和服务器地址后重试。",
                };
            }
            catch (OperationCanceledException) { statusText.Text = "请求已取消。"; }
            catch (Exception error) when (IsCoreUnavailable(error)) { statusText.Text = "Rust 核心未能加载，请重新构建或修复应用安装。"; }
            finally
            {
                FinishRequest();
                if (authenticated) dialog.Hide();
            }
        };

        try
        {
            await dialog.ShowAsync();
        }
        finally
        {
            requestCancellation?.Cancel();
            requestCancellation?.Dispose();
            passwordBox.Password = string.Empty;
            deviceOtpBox.Text = string.Empty;
            pendingDeviceOtp = string.Empty;
            tokenBox.Text = string.Empty;
            // Server preferences are not account credentials and can be saved without logging in.
            var (mode, url) = GetServerSelection();
            try
            {
                if (mode != 2 || RustCore.IsValidServer(mode, url))
                    new LoginEnvironment(mode, serverUrlBox.Text.Trim()).Save();
            }
            catch (Exception error) when (IsCoreUnavailable(error)) { }
        }
    }

    private static bool IsCoreUnavailable(Exception error) =>
        error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;

    private static void PopulateTwoFactorMethods(ComboBox providerBox, uint methodMask)
    {
        providerBox.Items.Clear();
        if ((methodMask & (1 << 0)) != 0)
        {
            providerBox.Items.Add(new ComboBoxItem { Content = "身份验证器", Tag = 0u });
        }
        if ((methodMask & (1 << 1)) != 0)
        {
            providerBox.Items.Add(new ComboBoxItem { Content = "邮箱验证码", Tag = 1u });
        }
        if ((methodMask & (1 << 3)) != 0)
        {
            providerBox.Items.Add(new ComboBoxItem { Content = "YubiKey", Tag = 3u });
        }
        if (providerBox.Items.Count > 0)
        {
            providerBox.SelectedIndex = 0;
        }
    }
}
