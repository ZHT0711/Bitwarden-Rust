using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace Bitwarden_WinUI;

/// <summary>
/// Provides application-specific behavior to supplement the default Application class.
/// </summary>
public partial class App : Application
{
    private MainWindow? _window;

    /// <summary>
    /// Initializes the singleton application object.  This is the first line of authored code
    /// executed, and as such is the logical equivalent of main() or WinMain().
    /// </summary>
    public App()
    {
        InitializeComponent();
    }

    /// <summary>
    /// Invoked when the application is launched.
    /// </summary>
    /// <param name="args">Details about the launch request and process.</param>
    protected override async void OnLaunched(Microsoft.UI.Xaml.LaunchActivatedEventArgs args)
    {
        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        var instance = AppInstance.FindOrRegisterForKey("primary");
        if (!instance.IsCurrent)
        {
            await instance.RedirectActivationToAsync(activation);
            Environment.Exit(0);
            return;
        }
        // Capture the UI dispatcher before handling redirected requests. The queue runs after
        // window construction, including when another launch races the first one.
        var dispatcher = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread();
        instance.Activated += (_, redirected) =>
        {
            if (!IsBackgroundLaunch(redirected))
                dispatcher.TryEnqueue(() => _window?.ShowMainWindow());
        };
        _window = new MainWindow();
        _window.Launch(IsBackgroundLaunch(activation) ||
            Environment.GetCommandLineArgs().Skip(1).Contains("--start-in-tray", StringComparer.Ordinal));
    }

    private static bool IsBackgroundLaunch(AppActivationArguments activation) =>
        activation.Kind == ExtendedActivationKind.StartupTask ||
        activation.Data is Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs { Arguments: "--start-in-tray" };
}
