using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

// Only the server choice is saved. Account credentials and authentication tokens are not stored.
internal readonly record struct LoginEnvironment(uint Mode, string CustomUrl)
{
    internal static LoginEnvironment Load()
    {
        try
        {
            var settings = ApplicationData.Current.LocalSettings.Values;
            var mode = settings.TryGetValue("LoginServerMode", out var storedMode) && storedMode is uint value
                ? value : 0u;
            var url = settings.TryGetValue("LoginCustomServer", out var storedUrl) && storedUrl is string text
                ? text : string.Empty;
            return new LoginEnvironment(mode <= 2 ? mode : 0, url);
        }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            return new LoginEnvironment(0, string.Empty);
        }
    }

    internal void Save()
    {
        try
        {
            var settings = ApplicationData.Current.LocalSettings.Values;
            settings["LoginServerMode"] = Mode;
            settings["LoginCustomServer"] = CustomUrl;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            // A local settings failure must not prevent login.
        }
    }
}
