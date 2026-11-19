using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
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

    private void SignIn_Click(object sender, RoutedEventArgs e)
    {
        if (NavFrame.Content is HomePage page)
        {
            page.ShowConnectionNotice();
        }
        else
        {
            NavFrame.Navigate(typeof(HomePage), "vault");
        }
    }
}
