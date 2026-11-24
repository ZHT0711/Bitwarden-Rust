using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal enum ClipboardClearResult { Cleared, Busy, Changed, Failed }

internal interface IClipboardClearAccess
{
    bool Open();
    uint Sequence { get; }
    bool Empty();
    void Close();
}

internal static class ClipboardClearTransaction
{
    // Caller has already verified its private ownership marker. The sequence check and
    // EmptyClipboard MUST share this native lock; a separate Clipboard.Clear has a race.
    internal static ClipboardClearResult TryClear(IClipboardClearAccess access, uint expectedSequence)
    {
        if (expectedSequence == 0) return ClipboardClearResult.Failed;
        if (!access.Open()) return ClipboardClearResult.Busy;
        try
        {
            if (access.Sequence != expectedSequence) return ClipboardClearResult.Changed;
            return access.Empty() ? ClipboardClearResult.Cleared : ClipboardClearResult.Failed;
        }
        finally { access.Close(); }
    }

    internal static bool IsRetryableReadFailure(Exception error) =>
        error is COMException or InvalidOperationException or OperationCanceledException or UnauthorizedAccessException;

    internal static TimeSpan RetryDelay(int attempt) => TimeSpan.FromMilliseconds(attempt < 12 ? 250 : 1000);
}

internal sealed class NativeClipboardClearAccess : IClipboardClearAccess
{
    internal const string OwnershipFormat = "BitwardenRust.ClipboardOwner";
    private static readonly uint OwnerFormat = RegisterClipboardFormat(OwnershipFormat);
    private readonly IntPtr _window;

    internal NativeClipboardClearAccess(IntPtr window)
    {
        if (window == IntPtr.Zero || GetWindowThreadProcessId(window, out var process) == 0 || process != (uint)Environment.ProcessId)
            throw new ArgumentException("Clipboard access requires a window owned by this process.", nameof(window));
        _window = window;
    }

    // NULL and the shared desktop HWND can both share/release another caller's lock.
    // Use the application's own live window, never a common global window handle.
    public bool Open() => OpenClipboard(_window);
    public uint Sequence => GetClipboardSequenceNumber();
    public bool Empty() => EmptyClipboard();
    public void Close()
    {
        if (!CloseClipboard()) throw new InvalidOperationException("Clipboard access could not be released.");
    }

    internal bool Probe()
    {
        if (!Open()) return false;
        Close();
        return true;
    }

    // WinRT may return an empty view during contention. Confirm that the native marker
    // is actually absent before forgetting an owned copy, not just absent from that view.
    internal bool? HasOwnerFormat()
    {
        if (OwnerFormat == 0) throw new InvalidOperationException("Clipboard ownership format is unavailable.");
        if (!Open()) return null;
        try { return IsClipboardFormatAvailable(OwnerFormat); }
        finally { Close(); }
    }

    [DllImport("user32.dll")] private static extern uint GetClipboardSequenceNumber();
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool CloseClipboard();
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool EmptyClipboard();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern uint RegisterClipboardFormat(string format);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool IsClipboardFormatAvailable(uint format);
}
