using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using System.Net;
using System.Threading.Tasks;
using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;

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

    private void NavFrame_Navigated(object sender, NavigationEventArgs e)
    {
        if (e.Content is HomePage page)
        {
            page.SignInRequested -= HomePage_SignInRequested;
            page.SignInRequested += HomePage_SignInRequested;
        }
    }

    private async void SignIn_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            if (RustCore.IsAuthenticated)
            {
                RustCore.Logout();
                SignInButton.Content = "登录";
                if (NavFrame.Content is HomePage homePage)
                {
                    homePage.ShowLoggedOut();
                }
                return;
            }
        }
        catch (DllNotFoundException)
        {
            return;
        }

        await ShowSignInDialogAsync();
    }

    private void HomePage_SignInRequested(object? sender, EventArgs e) => _ = ShowSignInDialogAsync();

    private async Task ShowSignInDialogAsync()
    {
        var serverBox = new ComboBox {
            Header = "服务器区域",
            HorizontalAlignment = HorizontalAlignment.Stretch,
            Items = {
                new ComboBoxItem { Content = "Bitwarden.com（美国）", Tag = 0u },
                new ComboBoxItem { Content = "Bitwarden.eu（欧盟）", Tag = 1u },
                new ComboBoxItem { Content = "自托管 / 自定义服务器", Tag = 2u },
            },
            SelectedIndex = 0,
        };
        var serverUrlBox = new TextBox {
            Header = "服务器 URL",
            PlaceholderText = "https://vault.example.com",
            Visibility = Visibility.Collapsed,
        };
        var emailBox = new TextBox { Header = "电子邮箱", PlaceholderText = "name@example.com" };
        var passwordBox = new PasswordBox { Header = "主密码", PlaceholderText = "输入 Bitwarden 主密码" };
        var providerBox = new ComboBox { Header = "二步验证方式", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Stretch };
        var tokenBox = new TextBox { Header = "验证码", PlaceholderText = "输入一次性验证码", Visibility = Visibility.Collapsed };
        var sendEmailButton = new Button { Content = "发送邮箱验证码", Visibility = Visibility.Collapsed, HorizontalAlignment = HorizontalAlignment.Left };
        var statusText = new TextBlock { TextWrapping = TextWrapping.Wrap };
        var content = new StackPanel { Spacing = 12, MinWidth = 360 };
        content.Children.Add(serverBox);
        content.Children.Add(serverUrlBox);
        content.Children.Add(emailBox);
        content.Children.Add(passwordBox);
        content.Children.Add(providerBox);
        content.Children.Add(tokenBox);
        content.Children.Add(sendEmailButton);
        content.Children.Add(statusText);

        var dialog = new ContentDialog
        {
            Title = "登录 Bitwarden",
            Content = content,
            PrimaryButtonText = "登录",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = AppTitleBar.XamlRoot,
        };

        providerBox.SelectionChanged += (_, _) =>
        {
            var isEmail = providerBox.SelectedItem is ComboBoxItem item && item.Tag is uint provider && provider == 1;
            sendEmailButton.Visibility = isEmail ? Visibility.Visible : Visibility.Collapsed;
        };

        serverBox.SelectionChanged += (_, _) =>
        {
            serverUrlBox.Visibility = serverBox.SelectedItem is ComboBoxItem item
                && item.Tag is uint mode
                && mode == 2
                    ? Visibility.Visible
                    : Visibility.Collapsed;
        };

        sendEmailButton.Click += async (_, _) =>
        {
            var email = emailBox.Text.Trim();
            var password = passwordBox.Password;
            var (serverMode, serverUrl) = GetServerSelection(serverBox, serverUrlBox);
            if (string.IsNullOrWhiteSpace(email) || string.IsNullOrEmpty(password))
            {
                statusText.Text = "先填写电子邮箱和主密码，再发送验证码。";
                return;
            }
            if (serverMode == 2 && !IsValidCustomServerUrl(serverUrl))
            {
                statusText.Text = "请输入有效的服务器地址，例如 https://vault.example.com；HTTP 仅支持本机回环地址。";
                return;
            }

            sendEmailButton.IsEnabled = false;
            statusText.Text = "正在请求发送验证码…";
            try
            {
                var sent = await Task.Run(() => RustCore.SendTwoFactorEmail(email, password, serverMode, serverUrl));
                statusText.Text = sent ? "验证码发送请求已成功。请检查邮箱。" : "无法发送验证码。请检查账户信息、网络连接后重试。";
            }
            catch (Exception)
            {
                statusText.Text = "无法发送验证码。请检查网络连接后重试。";
            }
            finally
            {
                sendEmailButton.IsEnabled = true;
            }
        };

        var unsupportedTwoFactor = false;
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            var deferral = args.GetDeferral();
            args.Cancel = true;
            dialog.IsPrimaryButtonEnabled = false;
            try
            {
                var email = emailBox.Text.Trim();
                var password = passwordBox.Password;
                var needsToken = tokenBox.Visibility == Visibility.Visible;
                var token = tokenBox.Text.Trim();
                var selectedProvider = providerBox.SelectedItem as ComboBoxItem;
                var providerId = selectedProvider?.Tag is uint provider ? provider : 0;
                var (serverMode, serverUrl) = GetServerSelection(serverBox, serverUrlBox);

                if (string.IsNullOrWhiteSpace(email) || string.IsNullOrEmpty(password))
                {
                    statusText.Text = "请填写电子邮箱和主密码。";
                    return;
                }
                if (serverMode == 2 && !IsValidCustomServerUrl(serverUrl))
                {
                    statusText.Text = "请输入有效的服务器地址，例如 https://vault.example.com；HTTP 仅支持本机回环地址。";
                    return;
                }
                if (needsToken && string.IsNullOrWhiteSpace(token))
                {
                    statusText.Text = "请输入二步验证码。";
                    return;
                }

                statusText.Text = "正在通过 Bitwarden SDK 验证…";
                var result = await Task.Run(() => RustCore.Login(email, password, providerId, token, serverMode, serverUrl));
                if (result.Status == 1)
                {
                    args.Cancel = false;
                    SignInButton.Content = "退出登录";
                    if (NavFrame.Content is HomePage page)
                    {
                        page.ShowAuthenticated();
                    }
                    return;
                }

                if (result.Status == 2)
                {
                    PopulateTwoFactorMethods(providerBox, result.AvailableTwoFactorMethods);
                    providerBox.Visibility = Visibility.Visible;
                    tokenBox.Visibility = Visibility.Visible;
                    if (providerBox.Items.Count == 0)
                    {
                        unsupportedTwoFactor = true;
                        dialog.IsPrimaryButtonEnabled = false;
                        statusText.Text = "账户要求的二步验证方式暂不受此预览版支持。当前已接入身份验证器、邮箱验证码和 YubiKey。";
                    }
                    else
                    {
                        dialog.IsPrimaryButtonEnabled = true;
                        statusText.Text = "请选择二步验证方式并输入验证码。";
                    }
                    return;
                }

                statusText.Text = "登录失败。请检查账户信息、验证码或网络连接后重试。";
            }
            catch (Exception)
            {
                statusText.Text = "登录暂时无法完成。请检查网络连接后重试。";
            }
            finally
            {
                if (args.Cancel && !unsupportedTwoFactor)
                {
                    dialog.IsPrimaryButtonEnabled = true;
                }
                deferral.Complete();
            }
        };

        try
        {
            await dialog.ShowAsync();
        }
        finally
        {
            passwordBox.Password = string.Empty;
            tokenBox.Text = string.Empty;
        }
    }

    private static (uint Mode, string Url) GetServerSelection(ComboBox serverBox, TextBox serverUrlBox)
    {
        var mode = serverBox.SelectedItem is ComboBoxItem item && item.Tag is uint selectedMode
            ? selectedMode
            : 0u;
        return (mode, mode == 2 ? serverUrlBox.Text.Trim() : string.Empty);
    }

    private static bool IsValidCustomServerUrl(string value)
    {
        if (!Uri.TryCreate(value.Trim(), UriKind.Absolute, out var uri)
            || string.IsNullOrEmpty(uri.Host)
            || !string.IsNullOrEmpty(uri.UserInfo)
            || !string.IsNullOrEmpty(uri.Query)
            || !string.IsNullOrEmpty(uri.Fragment))
        {
            return false;
        }

        var isLoopback = uri.Host.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || IPAddress.TryParse(uri.Host, out var address) && IPAddress.IsLoopback(address);
        if (uri.Scheme != Uri.UriSchemeHttps && !(uri.Scheme == Uri.UriSchemeHttp && isLoopback))
        {
            return false;
        }

        var lastSegment = uri.AbsolutePath.TrimEnd('/').Split('/').LastOrDefault();
        return lastSegment is not ("api" or "identity");
    }

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
