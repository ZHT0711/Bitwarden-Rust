namespace Bitwarden_WinUI.Interop;

// UI-thread only. Batch queued input without a fixed typing debounce; retain no old page on cancel.
internal sealed class SearchDispatch(Func<Action, bool> enqueue, Action<object, string> apply)
{
    private object? _pendingTarget, _appliedTarget;
    private string _pendingText = "", _appliedText = "";
    private bool _queued;
    private long _generation;

    internal void Request(object target, string text)
    {
        _pendingTarget = target;
        _pendingText = text;
        if (_queued) return;
        if (ReferenceEquals(_appliedTarget, target) && _appliedText == text)
        {
            _pendingTarget = null;
            _pendingText = "";
            return;
        }
        _queued = true;
        var generation = _generation;
        if (!enqueue(() => Drain(generation))) Drain(generation);
    }

    internal void Cancel()
    {
        _generation++;
        _queued = false;
        _pendingTarget = _appliedTarget = null;
        _pendingText = _appliedText = "";
    }

    private void Drain(long generation)
    {
        if (generation != _generation) return;
        _queued = false;
        var target = _pendingTarget;
        var text = _pendingText;
        _pendingTarget = null;
        _pendingText = "";
        if (target is null || (ReferenceEquals(_appliedTarget, target) && _appliedText == text)) return;
        apply(target, text);
        if (generation != _generation) return;
        _appliedTarget = target;
        _appliedText = text;
    }
}
