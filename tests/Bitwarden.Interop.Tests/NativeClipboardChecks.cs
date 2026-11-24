using Bitwarden_WinUI.Interop;
using System.Runtime.InteropServices;
using System.Text;

internal static class NativeClipboardChecks
{
    // Explicit local-only opt-in: writes public synthetic strings to the interactive Windows
    // clipboard. Never reads a pre-existing clipboard or emits its contents to a test log.
    internal static void Run(Action<bool, string> check)
    {
        if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException("Native clipboard checks require Windows.");
        foreach (var invalid in new[] { IntPtr.Zero, GetDesktopWindow() })
        {
            try { _ = new NativeClipboardClearAccess(invalid); throw new Exception("An unsafe clipboard owner window was accepted."); }
            catch (ArgumentException) { check(true, $"native clipboard rejects {(invalid == IntPtr.Zero ? "anonymous" : "shared desktop")} transaction ownership"); }
        }
        var window = CreateWindowEx(0, "STATIC", "BitwardenRustSyntheticClipboardCheck", 0, 0, 0, 0, 0, new IntPtr(-3), IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
        if (window == IntPtr.Zero) throw new Exception("Cannot create the synthetic clipboard message window.");
        var native = new NativeClipboardClearAccess(window);
        try
        {
            Write("Synthetic clipboard transaction");
            var sequence = native.Sequence;
            var result = TryClearWhenAvailable(native, sequence);
            var hasText = HasText();
            if (result != ClipboardClearResult.Cleared || hasText)
                Console.WriteLine($"clipboard diagnostic: expected-sequence={sequence} now-sequence={native.Sequence} result={result} has-text={hasText}");
            check(sequence > 0 && result == ClipboardClearResult.Cleared && !hasText,
                "actual Windows clipboard transaction clears its matching synthetic snapshot");

            Write("Synthetic clipboard first owner");
            sequence = native.Sequence;
            Write("Synthetic clipboard next writer");
            check(TryClearWhenAvailable(native, sequence) == ClipboardClearResult.Changed && Read() == "Synthetic clipboard next writer",
                "actual Windows sequence race preserves a writer arriving before the native lock");

            foreach (var anonymous in new[] { true, false })
            {
                Write("Synthetic clipboard guarded writer");
                var blocked = false;
                var guarded = new ObservedAccess(native, () =>
                {
                    var writer = new Thread(() =>
                    {
                        var opened = OpenClipboard(anonymous ? IntPtr.Zero : GetDesktopWindow());
                        blocked = !opened;
                        if (opened) CloseClipboard();
                    });
                    writer.Start();
                    if (!writer.Join(3000)) throw new Exception("Competing writer did not return.");
                });
                check(TryClearWhenAvailable(guarded, native.Sequence) == ClipboardClearResult.Cleared && blocked,
                    $"actual competing {(anonymous ? "anonymous" : "desktop-owner")} native thread is excluded between sequence check and empty");
            }

            Write("Synthetic clipboard prior snapshot");
            var guard = new ObservedAccess(native, null, () => Write("Synthetic clipboard writer after unlock"));
            check(TryClearWhenAvailable(guard, native.Sequence) == ClipboardClearResult.Cleared && Read() == "Synthetic clipboard writer after unlock",
                "actual writer after transaction unlock is preserved without a second clear");

            Write("Synthetic clipboard contention");
            using var openedSignal = new ManualResetEventSlim();
            using var release = new ManualResetEventSlim();
            Exception? failure = null;
            var holder = new Thread(() =>
            {
                try
                {
                    if (!OpenClipboard(IntPtr.Zero)) throw new Exception("Cannot hold synthetic clipboard.");
                    openedSignal.Set();
                    try { if (!release.Wait(5000)) throw new Exception("Synthetic clipboard holder timed out."); }
                    finally { CloseClipboard(); }
                }
                catch (Exception error) { failure = error; openedSignal.Set(); }
            });
            sequence = native.Sequence;
            holder.Start();
            try
            {
                if (!openedSignal.Wait(3000) || failure is not null) throw new Exception("Clipboard holder did not become ready.", failure);
                check(ClipboardClearTransaction.TryClear(native, sequence) == ClipboardClearResult.Busy,
                    "actual busy clipboard transaction does not share or release another thread's anonymous lock");
            }
            finally { release.Set(); if (!holder.Join(3000)) throw new Exception("Clipboard holder did not release."); }
            if (failure is not null) throw failure;
            check(TryClearWhenAvailable(native, sequence) == ClipboardClearResult.Cleared && !HasText(),
                "actual native clear succeeds once the external clipboard lock is released");
        }
        finally
        {
            try
            {
                if (native.Open()) { try { native.Empty(); } finally { native.Close(); } }
            }
            finally { DestroyWindow(window); }
        }
    }

    private sealed class ObservedAccess(IClipboardClearAccess native, Action? beforeEmpty = null, Action? afterClose = null) : IClipboardClearAccess
    {
        public bool Open() => native.Open();
        public uint Sequence => native.Sequence;
        public bool Empty() { beforeEmpty?.Invoke(); return native.Empty(); }
        public void Close() { native.Close(); afterClose?.Invoke(); }
    }

    private static ClipboardClearResult TryClearWhenAvailable(IClipboardClearAccess access, uint sequence)
    {
        var deadline = DateTime.UtcNow.AddSeconds(2);
        while (true)
        {
            var result = ClipboardClearTransaction.TryClear(access, sequence);
            if (result != ClipboardClearResult.Busy || DateTime.UtcNow >= deadline) return result;
            Thread.Sleep(10); // Native clipboard listeners can briefly contend after a write.
        }
    }

    private static void OpenEventually()
    {
        var deadline = DateTime.UtcNow.AddSeconds(2);
        while (!OpenClipboard(GetDesktopWindow()))
        {
            if (DateTime.UtcNow >= deadline) throw new Exception("Synthetic clipboard access timed out.");
            Thread.Sleep(10);
        }
    }

    private static bool HasText()
    {
        OpenEventually();
        try { return IsClipboardFormatAvailable(13); }
        finally { CloseClipboard(); }
    }

    private static string Read()
    {
        OpenEventually();
        try
        {
            var handle = GetClipboardData(13);
            var pointer = GlobalLock(handle);
            if (pointer == IntPtr.Zero) throw new Exception("Synthetic clipboard text is unavailable.");
            try { return Marshal.PtrToStringUni(pointer) ?? ""; }
            finally { GlobalUnlock(handle); }
        }
        finally { CloseClipboard(); }
    }

    private static void Write(string text)
    {
        OpenEventually();
        try
        {
            if (!EmptyClipboard()) throw new Exception("Cannot empty synthetic clipboard.");
            Install(RegisterClipboardFormat("CanIncludeInClipboardHistory"), new byte[4]);
            Install(RegisterClipboardFormat("CanUploadToCloudClipboard"), new byte[4]);
            Install(13, Encoding.Unicode.GetBytes(text + '\0'));
        }
        finally { CloseClipboard(); }
    }

    private static void Install(uint format, byte[] bytes)
    {
        if (format == 0) throw new Exception("Cannot register synthetic clipboard format.");
        var handle = GlobalAlloc(0x42, (nuint)bytes.Length);
        if (handle == IntPtr.Zero) throw new Exception("Cannot allocate synthetic clipboard data.");
        var transferred = false;
        try
        {
            var pointer = GlobalLock(handle);
            if (pointer == IntPtr.Zero) throw new Exception("Cannot access synthetic clipboard allocation.");
            try { Marshal.Copy(bytes, 0, pointer, bytes.Length); }
            finally { GlobalUnlock(handle); }
            if (SetClipboardData(format, handle) == IntPtr.Zero) throw new Exception("Cannot install synthetic clipboard data.");
            transferred = true;
        }
        finally { if (!transferred) GlobalFree(handle); }
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr CreateWindowEx(uint extended, string type, string title, uint style, int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool DestroyWindow(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern uint RegisterClipboardFormat(string format);
    [DllImport("user32.dll")] private static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")] private static extern bool CloseClipboard();
    [DllImport("user32.dll")] private static extern bool EmptyClipboard();
    [DllImport("user32.dll")] private static extern bool IsClipboardFormatAvailable(uint format);
    [DllImport("user32.dll")] private static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll")] private static extern IntPtr SetClipboardData(uint format, IntPtr handle);
    [DllImport("user32.dll")] private static extern IntPtr GetDesktopWindow();
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalAlloc(uint flags, nuint size);
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalFree(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern IntPtr GlobalLock(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern bool GlobalUnlock(IntPtr handle);
}
