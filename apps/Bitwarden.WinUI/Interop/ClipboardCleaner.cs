using Microsoft.UI.Dispatching;
using Windows.ApplicationModel.DataTransfer;
using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static class ClipboardCleaner
{
    private static string? _owner;
    private static NativeClipboardClearAccess? _native;
    private static DispatcherQueueTimer? _timer;
    private static TaskCompletionSource<bool>? _pending;
    private static int _attempt;
    private static bool _shuttingDown;
    private static bool _stopped;
    internal static event Action? Copied;

    internal static void Initialize(IntPtr window) => _native = new NativeClipboardClearAccess(window);

    internal static void Copy(string value)
    {
        if (_native is null || _shuttingDown || _stopped) throw new InvalidOperationException("Clipboard access is unavailable.");
        var package = new DataPackage();
        var owner = Guid.NewGuid().ToString();
        package.SetText(value);
        package.SetData(NativeClipboardClearAccess.OwnershipFormat, owner);
        if (!Clipboard.SetContentWithOptions(package, new ClipboardContentOptions { IsAllowedInHistory = false, IsRoamable = false }))
            throw new InvalidOperationException("Clipboard could not be set.");
        StopTimer();
        _pending?.TrySetResult(false); // A new copy invalidates every old async read and retry.
        _pending = null;
        _owner = owner;
        var seconds = DesktopPreferences.ReadClipboardDelay();
        if (seconds > 0) Schedule(owner, TimeSpan.FromSeconds(seconds), () => ClearOwned(owner));
        Copied?.Invoke();
    }

    internal static void ClearOwned(string? expectedOwner = null) => _ = ClearOwnedAsync(expectedOwner);

    private static Task<bool> ClearOwnedAsync(string? expectedOwner = null)
    {
        var expected = expectedOwner ?? _owner;
        if (expected is null) return Task.FromResult(true);
        if (expected != _owner || _stopped) return Task.FromResult(false);
        if (_pending is not null) return _pending.Task;
        StopTimer();
        _attempt = 0;
        _pending = new(TaskCreationOptions.RunContinuationsAsynchronously);
        var task = _pending.Task;
        _ = ClearOwnedAttemptAsync(expected);
        return task;
    }

    internal static async Task<bool> ClearForExitAsync(TimeSpan timeout)
    {
        _shuttingDown = true; // Do not accept a new secret while the window is waiting to close.
        try { return await ClearOwnedAsync().WaitAsync(timeout); }
        catch (TimeoutException) { return false; } // Windows contention cannot delay exit forever.
    }

    internal static void Stop()
    {
        _stopped = true;
        StopTimer();
        _pending?.TrySetResult(false);
        _pending = null;
        _owner = null;
        _native = null;
    }

    private static async Task ClearOwnedAttemptAsync(string expected)
    {
        if (_owner != expected || _stopped) return;
        try
        {
            var native = _native ?? throw new InvalidOperationException("Clipboard access is unavailable.");
            if (!native.Probe()) { Retry(expected); return; }
            var sequence = native.Sequence;
            var content = Clipboard.GetContent();
            if (!content.Contains(NativeClipboardClearAccess.OwnershipFormat))
            {
                if (native.HasOwnerFormat() == false) Finish(expected);
                else Retry(expected); // An empty WinRT view is not proof that the copy is gone.
                return;
            }
            var marker = await content.GetDataAsync(NativeClipboardClearAccess.OwnershipFormat);
            if (_owner != expected || _stopped) return;
            if (marker is not string owner || owner != expected) { Finish(expected); return; }

            // No await between the final owner check and the native transaction on this UI
            // thread. Other processes are excluded by OpenClipboard for check + empty.
            if (ClipboardClearTransaction.TryClear(native, sequence) == ClipboardClearResult.Cleared)
                Finish(expected);
            else Retry(expected); // Re-read the marker after a sequence change or busy lock.
        }
        catch (Exception error) when (ClipboardClearTransaction.IsRetryableReadFailure(error))
        {
            Retry(expected);
        }
    }

    private static void Retry(string expected)
    {
        if (_owner != expected || _stopped) return;
        try
        {
            var delay = ClipboardClearTransaction.RetryDelay(_attempt);
            if (_attempt < 12) _attempt++;
            Schedule(expected, delay, () => _ = ClearOwnedAttemptAsync(expected));
        }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            StopTimer();
            _pending?.TrySetResult(false);
            _pending = null; // A later lock/exit may try again if the dispatcher is still alive.
        }
    }

    private static void Finish(string expected)
    {
        if (_owner != expected) return;
        StopTimer();
        _owner = null;
        _pending?.TrySetResult(true);
        _pending = null;
    }

    private static void Schedule(string expected, TimeSpan delay, Action action)
    {
        StopTimer();
        var dispatcher = DispatcherQueue.GetForCurrentThread() ?? throw new InvalidOperationException("Clipboard dispatcher is unavailable.");
        var timer = dispatcher.CreateTimer();
        _timer = timer;
        timer.Interval = delay;
        timer.IsRepeating = false;
        timer.Tick += (_, _) =>
        {
            if (_stopped || _owner != expected || !ReferenceEquals(_timer, timer)) return;
            _timer = null;
            action();
        };
        timer.Start();
    }

    private static void StopTimer()
    {
        _timer?.Stop();
        _timer = null;
    }
}
