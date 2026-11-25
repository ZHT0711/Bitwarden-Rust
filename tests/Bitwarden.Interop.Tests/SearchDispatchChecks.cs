using Bitwarden_WinUI.Interop;

internal static class SearchDispatchChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var pending = new Queue<Action>();
        var applied = new List<(object Target, string Text)>();
        var dispatch = new SearchDispatch(action => { pending.Enqueue(action); return true; },
            (target, text) => applied.Add((target, text)));
        var first = new object();
        var second = new object();
        for (var index = 0; index < 21; index++) dispatch.Request(first, $"prefix-{index}");
        check(pending.Count == 1 && applied.Count == 0, "search batches queued input without applying intermediate prefixes");
        pending.Dequeue()();
        check(applied.Count == 1 && applied[0] == (first, "prefix-20"), "search dispatch applies the latest complete text once");
        dispatch.Request(first, "prefix-20");
        check(pending.Count == 0, "delayed public AutoSuggestBox duplicate does not reapply raw input");
        dispatch.Request(first, "PREFIX-20");
        pending.Dequeue()();
        check(applied[^1].Text == "PREFIX-20", "search preserves exact text rather than normalizing query syntax");
        dispatch.Request(first, "other");
        dispatch.Request(first, "PREFIX-20");
        pending.Dequeue()();
        check(applied.Count == 2, "typing back to the applied query avoids unnecessary list work");
        dispatch.Request(second, "PREFIX-20");
        pending.Dequeue()();
        check(applied.Count == 3 && ReferenceEquals(applied[^1].Target, second), "same text on a different page still updates its projection");
        dispatch.Request(first, "old-page");
        dispatch.Cancel();
        dispatch.Request(second, "new-page");
        pending.Dequeue()();
        check(applied.Count == 3, "navigation cancellation invalidates already queued old-page callbacks");
        pending.Dequeue()();
        check(applied.Count == 4 && applied[^1] == (second, "new-page"), "cancelled old callback cannot drain a newer page request");
        dispatch.Request(first, "pending-lock");
        dispatch.Cancel();
        pending.Dequeue()();
        check(applied.Count == 4, "cancelled input cannot reapply after lock/exit lifecycle invalidation");
        dispatch.Request(second, "new-page");
        pending.Dequeue()();
        check(applied.Count == 5, "cancellation resets the dedupe cache for a refreshed page lifecycle");
        string? immediate = null;
        var refused = new SearchDispatch(_ => false, (_, text) => immediate = text);
        refused.Request(first, "queue-refused");
        check(immediate == "queue-refused", "queue refusal falls back to synchronous search rather than losing input");
        SearchDispatch? reentrant = null;
        var calls = 0;
        reentrant = new SearchDispatch(action => { pending.Enqueue(action); return true; }, (_, _) =>
        {
            calls++;
            reentrant!.Cancel();
        });
        reentrant.Request(first, "cancel-during-apply");
        pending.Dequeue()();
        reentrant.Request(first, "cancel-during-apply");
        check(pending.Count == 1, "cancellation during apply must not repopulate an old page cache");
        pending.Dequeue()();
        check(calls == 2, "reentrant lifecycle cancellation permits the next identical request");
    }
}
