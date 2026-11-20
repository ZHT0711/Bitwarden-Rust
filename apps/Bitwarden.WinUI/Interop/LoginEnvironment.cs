using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

// The server choice and a non-secret installation identifier are saved. Credentials and tokens are not stored.
internal readonly record struct LoginEnvironment(uint Mode, string CustomUrl)
{
    internal static string GetDeviceIdentifier()
    {
        const string key = "LoginDeviceIdentifier";
        try
        {
            var settings = ApplicationData.Current.LocalSettings.Values;
            if (settings.TryGetValue(key, out var stored) && stored is string value && Guid.TryParse(value, out var id))
                return id.ToString("D");
            var created = Guid.NewGuid().ToString("D");
            settings[key] = created;
            return created;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            return Guid.NewGuid().ToString("D");
        }
    }

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
