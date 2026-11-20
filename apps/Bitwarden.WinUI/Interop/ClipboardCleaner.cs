using Microsoft.UI.Dispatching;
using Windows.ApplicationModel.DataTransfer;
using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static class ClipboardCleaner
{
    private const string OwnershipFormat = "BitwardenRust.ClipboardOwner";
    private static string? _owner;
    private static DispatcherQueueTimer? _timer;
    [DllImport("user32.dll")]
    private static extern uint GetClipboardSequenceNumber();

    internal static void Copy(string value)
    {
        var package = new DataPackage();
        var owner = Guid.NewGuid().ToString();
        package.SetText(value);
        package.SetData(OwnershipFormat, owner);
        if (!Clipboard.SetContentWithOptions(package, new ClipboardContentOptions { IsAllowedInHistory = false, IsRoamable = false }))
            throw new InvalidOperationException("Clipboard could not be set.");
        _owner = owner;
        _timer?.Stop();
        _timer = DispatcherQueue.GetForCurrentThread().CreateTimer();
        _timer.Interval = TimeSpan.FromSeconds(30);
        _timer.IsRepeating = false;
        _timer.Tick += (_, _) => ClearOwned(owner);
        _timer.Start();
    }

    internal static async void ClearOwned(string? expectedOwner = null)
    {
        if (expectedOwner is not null && expectedOwner != _owner) return;
        _timer?.Stop();
        try
        {
            var sequence = GetClipboardSequenceNumber();
            var content = Clipboard.GetContent();
            var expected = _owner;
            if (expected is not null && content.Contains(OwnershipFormat) &&
                await content.GetDataAsync(OwnershipFormat) is string owner && owner == expected &&
                _owner == expected && GetClipboardSequenceNumber() == sequence)
            {
                Clipboard.Clear();
                _owner = null;
            }
        }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
        {
            // Do not replace another application's clipboard or crash when the clipboard is busy.
        }
    }
}
