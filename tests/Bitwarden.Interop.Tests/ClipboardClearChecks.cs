using Bitwarden_WinUI.Interop;

internal static class ClipboardClearChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var access = new FakeAccess { SequenceValue = 17 };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Cleared && access.Emptied,
            "owned clipboard transaction clears a matching sequence");
        check(access.Calls.SequenceEqual(["open", "sequence", "empty", "close"]) && !access.Locked,
            "clipboard sequence check and empty share one native lock with guaranteed close");
        access = new() { SequenceValue = 18 };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Changed && !access.Emptied && !access.Locked,
            "clipboard transaction preserves a subsequent writer and releases the lock");
        access = new() { CanOpen = false };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Busy && access.Calls.SequenceEqual(["open"]),
            "busy clipboard is not read, emptied or accidentally closed");
        access = new();
        check(ClipboardClearTransaction.TryClear(access, 0) == ClipboardClearResult.Failed && access.Calls.Count == 0,
            "unavailable zero clipboard sequence cannot authorize a clear");
        access = new() { SequenceValue = 17, CanEmpty = false };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Failed && !access.Emptied && !access.Locked,
            "failed native empty releases its lock without reporting success");
        foreach (var throwOn in new[] { "sequence", "empty" })
        {
            access = new() { SequenceValue = 17, ThrowOn = throwOn };
            try { ClipboardClearTransaction.TryClear(access, 17); throw new Exception("Expected injected failure."); }
            catch (InvalidOperationException) { check(!access.Locked && access.Calls[^1] == "close", $"clipboard {throwOn} failure still closes the native transaction"); }
        }
        access = new() { SequenceValue = 17, OnOpen = fake => fake.SequenceValue++ };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Changed && !access.Emptied,
            "writer racing immediately before OpenClipboard cannot be erased");
        access = new() { SequenceValue = 17, OnEmpty = fake => fake.WriterBlocked = fake.Locked };
        check(ClipboardClearTransaction.TryClear(access, 17) == ClipboardClearResult.Cleared && access.WriterBlocked,
            "a writer between sequence check and empty is excluded by the held lock");
        var random = new Random(194061);
        var valid = true;
        for (var iteration = 0; iteration < 10_000; iteration++)
        {
            var sequence = (uint)random.Next(1, int.MaxValue - 1);
            var raced = random.Next(2) == 0;
            access = new() { SequenceValue = sequence, OnOpen = fake => { if (raced) fake.SequenceValue++; } };
            var result = ClipboardClearTransaction.TryClear(access, sequence);
            valid &= result == (raced ? ClipboardClearResult.Changed : ClipboardClearResult.Cleared) && access.Emptied != raced && !access.Locked;
        }
        check(valid, "10000 seeded sequence races clear only the still-owned clipboard snapshot");
        check(ClipboardClearTransaction.RetryDelay(0) == TimeSpan.FromMilliseconds(250) &&
            ClipboardClearTransaction.RetryDelay(11) == TimeSpan.FromMilliseconds(250), "clipboard contention retries promptly for the first three seconds");
        check(ClipboardClearTransaction.RetryDelay(12) == TimeSpan.FromSeconds(1) &&
            ClipboardClearTransaction.RetryDelay(int.MaxValue) == TimeSpan.FromSeconds(1), "long clipboard contention backs off without overflow or abandoning cleanup");
        Exception[] readFailures = [new System.Runtime.InteropServices.COMException(), new InvalidOperationException(),
            new ObjectDisposedException("synthetic clipboard"), new OperationCanceledException(), new TaskCanceledException(), new UnauthorizedAccessException()];
        check(readFailures.All(ClipboardClearTransaction.IsRetryableReadFailure),
            "clipboard read cancellation, revoked views and access failures keep the cleanup request retryable");
        check(!ClipboardClearTransaction.IsRetryableReadFailure(new ArgumentException()) &&
            !ClipboardClearTransaction.IsRetryableReadFailure(new NullReferenceException()),
            "clipboard retry classification does not silently absorb unrelated programming errors");
    }

    private sealed class FakeAccess : IClipboardClearAccess
    {
        internal uint SequenceValue;
        internal bool CanOpen = true;
        internal bool CanEmpty = true;
        internal bool Locked;
        internal bool Emptied;
        internal bool WriterBlocked;
        internal string? ThrowOn;
        internal Action<FakeAccess>? OnOpen;
        internal Action<FakeAccess>? OnEmpty;
        internal List<string> Calls = [];
        public bool Open()
        {
            Calls.Add("open");
            OnOpen?.Invoke(this);
            return Locked = CanOpen;
        }
        public uint Sequence
        {
            get
            {
                if (!Locked) throw new Exception("Sequence was read outside its transaction.");
                Calls.Add("sequence");
                if (ThrowOn == "sequence") throw new InvalidOperationException("Injected sequence failure.");
                return SequenceValue;
            }
        }
        public bool Empty()
        {
            if (!Locked) throw new Exception("Empty was called outside its transaction.");
            Calls.Add("empty");
            OnEmpty?.Invoke(this);
            if (ThrowOn == "empty") throw new InvalidOperationException("Injected empty failure.");
            return Emptied = CanEmpty;
        }
        public void Close()
        {
            if (!Locked) throw new Exception("An unowned clipboard lock was closed.");
            Calls.Add("close");
            Locked = false;
        }
    }
}
