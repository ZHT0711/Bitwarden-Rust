using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bytewarden.WinUI.Interop;

internal static class DesktopPreferences
{
    internal const string StartupTaskId = "BytewardenStartup";
    internal const string KeepInTray = "Desktop.KeepInTray";
    internal const string StartHidden = "Desktop.StartHidden";
    private const string Theme = "Desktop.Theme";
    private const string VaultSort = "Desktop.VaultSort";
    private const string ClipboardDelay = "Desktop.ClipboardSeconds";
    internal const string MinimizeOnCopy = "Desktop.MinimizeOnCopy";
    internal const string ShowQuickCopyActions = "Desktop.ShowQuickCopyActions";
    internal const string PreventScreenshots = "Desktop.PreventScreenshots";
    internal const string MinimizeToTray = "Desktop.MinimizeToTray";
    internal const string AutoPromptHello = "Desktop.AutoPromptHello";
    private const string WindowX = "Desktop.WindowX";
    private const string WindowY = "Desktop.WindowY";
    private const string WindowWidth = "Desktop.WindowWidth";
    private const string WindowHeight = "Desktop.WindowHeight";
    private const string WindowMaximized = "Desktop.WindowMaximized";
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

    internal static int ReadVaultSort()
    {
        var stored = ReadValue(VaultSort, 0);
        return stored is >= 0 and <= 3 ? stored : 0;
    }

    internal static bool TrySaveVaultSort(int sort) => sort is >= 0 and <= 3 && TrySaveValue(VaultSort, sort);

    internal static int ReadClipboardDelay()
    {
        var stored = ReadValue(ClipboardDelay, DefaultClipboardSeconds);
        return ClipboardChoices.Contains(stored) ? stored : DefaultClipboardSeconds;
    }

    internal static bool TrySaveClipboardDelay(int seconds) => ClipboardChoices.Contains(seconds) && TrySaveValue(ClipboardDelay, seconds);

    internal static (int X, int Y, int Width, int Height, bool Maximized)? ReadWindowBounds()
    {
        try
        {
            var values = ApplicationData.Current.LocalSettings.Values;
            if (values.TryGetValue(WindowX, out var rawX) && rawX is int x &&
                values.TryGetValue(WindowY, out var rawY) && rawY is int y &&
                values.TryGetValue(WindowWidth, out var rawW) && rawW is int width && width >= 320 &&
                values.TryGetValue(WindowHeight, out var rawH) && rawH is int height && height >= 240)
            {
                var maximized = values.TryGetValue(WindowMaximized, out var rawMax) && rawMax is true;
                return (x, y, width, height, maximized);
            }
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { }
        return null;
    }

    internal static bool TrySaveWindowBounds(int x, int y, int width, int height, bool maximized)
    {
        if (width < 320 || height < 240) return false;
        try
        {
            var values = ApplicationData.Current.LocalSettings.Values;
            values[WindowX] = x;
            values[WindowY] = y;
            values[WindowWidth] = width;
            values[WindowHeight] = height;
            values[WindowMaximized] = maximized;
            return true;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { return false; }
    }

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
