using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Bitwarden_WinUI.Pages;

public sealed partial class HomePage : Page
{
    public event EventHandler? SignInRequested;

    public HomePage()
    {
        InitializeComponent();
        try
        {
            CoreStatus.Text = $"Rust 核心版本 {RustCore.Version} 已加载。";
            if (RustCore.IsAuthenticated)
            {
                ShowAuthenticated();
            }
        }
        catch (DllNotFoundException)
        {
            CoreStatus.Text = "未找到 Rust 核心 DLL；请从仓库根目录构建应用。";
        }
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        var category = e.Parameter as string ?? "vault";
        (PageTitle.Text, PageSubtitle.Text) = category switch
        {
            "favorites" => ("收藏夹", "你标记为收藏的保险库项目会显示在这里。"),
            "logins" => ("登录", "网站和应用的账号凭据。"),
            "notes" => ("安全笔记", "安全保存文本和重要信息。"),
            "cards" => ("银行卡", "管理支付卡片信息。"),
            "identities" => ("身份信息", "管理个人身份与地址信息。"),
            "trash" => ("回收站", "查看已删除的保险库项目。"),
            _ => ("我的保险库", "密码、笔记与敏感信息都集中在这里。"),
        };
        base.OnNavigatedTo(e);
    }

    public void SetSearch(string query)
    {
        if (!string.IsNullOrWhiteSpace(query))
        {
            ConnectionNotice.Title = "搜索尚未连接保险库";
            ConnectionNotice.Message = $"收到搜索：{query}。本地条目索引会在账户和加密存储接入后启用。";
        }
        ConnectionNotice.Severity = InfoBarSeverity.Informational;
        ConnectionNotice.IsOpen = true;
    }

    public void ShowAuthenticated()
    {
        ConnectionNotice.Title = "账户认证成功";
        ConnectionNotice.Message = "已通过 Bitwarden Rust SDK 完成身份认证。保险库同步和条目加载仍在开发中。";
        ConnectionNotice.Severity = InfoBarSeverity.Success;
        ConnectionNotice.IsOpen = true;
    }

    public void ShowLoggedOut()
    {
        ConnectionNotice.Title = "账户已退出";
        ConnectionNotice.Message = "当前没有活跃的账户会话。";
        ConnectionNotice.Severity = InfoBarSeverity.Informational;
        ConnectionNotice.IsOpen = true;
    }

    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => SignInRequested?.Invoke(this, EventArgs.Empty);

    private void OpenGenerator_Click(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(GeneratorPage));
}
