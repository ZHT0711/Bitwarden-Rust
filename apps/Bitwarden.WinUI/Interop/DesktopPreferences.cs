using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

internal static class DesktopPreferences
{
    internal const string StartupTaskId = "BitwardenRustStartup";
    internal const string KeepInTray = "Desktop.KeepInTray";
    internal const string StartHidden = "Desktop.StartHidden";
    private const string Theme = "Desktop.Theme";
    private const string ClipboardDelay = "Desktop.ClipboardSeconds";
    internal const string MinimizeOnCopy = "Desktop.MinimizeOnCopy";
    internal static readonly IReadOnlyList<int> ClipboardChoices = Array.AsReadOnly(new[] { 0, 10, 20, 30, 60, 120, 300 });
    internal const int DefaultClipboardSeconds = 30;

    internal static bool Read(string key, bool fallback = true) => ReadValue(key, fallback);
    internal static bool TrySave(string key, bool enabled) => TrySaveValue(key, enabled);

    internal static int ReadTheme()
    {
        var stored = ReadValue(Theme, 0);
        return stored is >= 0 and <= 2 ? stored : 0;
    }

    internal static bool TrySaveTheme(int theme) => theme is >= 0 and <= 2 && TrySaveValue(Theme, theme);

    internal static int ReadClipboardDelay()
    {
        var stored = ReadValue(ClipboardDelay, DefaultClipboardSeconds);
        return ClipboardChoices.Contains(stored) ? stored : DefaultClipboardSeconds;
    }

    internal static bool TrySaveClipboardDelay(int seconds) => ClipboardChoices.Contains(seconds) && TrySaveValue(ClipboardDelay, seconds);

    private static T ReadValue<T>(string key, T fallback)
    {
        try { return ApplicationData.Current.LocalSettings.Values.TryGetValue(key, out var value) && value is T typed ? typed : fallback; }
        catch (Exception error) when (error is COMException or InvalidOperationException) { return fallback; }
    }

    private static bool TrySaveValue<T>(string key, T value) where T : notnull
    {
        try { ApplicationData.Current.LocalSettings.Values[key] = value; return true; }
        catch (Exception error) when (error is COMException or InvalidOperationException) { return false; }
    }
}
