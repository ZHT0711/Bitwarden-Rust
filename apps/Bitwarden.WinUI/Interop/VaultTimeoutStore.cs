using System.Runtime.InteropServices;
using System.Text.Json;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

internal static class VaultTimeoutStore
{
    internal static VaultTimeoutSettings Load(string key)
    {
        try {
            if (ApplicationData.Current.LocalSettings.Values.TryGetValue(key, out var stored) && stored is string json) {
                var settings = JsonSerializer.Deserialize<VaultTimeoutSettings>(json);
                if (settings.IsValid) return settings;
            }
        }
        catch (Exception error) when (error is COMException or InvalidOperationException or JsonException) { }
        return VaultTimeoutSettings.Default;
    }

    internal static bool TrySave(string key, VaultTimeoutSettings settings)
    {
        if (!settings.IsValid) return false;
        try {
            ApplicationData.Current.LocalSettings.Values[key] = JsonSerializer.Serialize(settings);
            return true;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { return false; }
    }
}
