using System.Diagnostics;
using System.IO;
using System.Text.Json;
using System.Windows.Automation;

internal static class Program
{
    private static int Main(string[] args)
    {
        if (args.Length is < 3 or > 4 || (args.Length == 4 && args[3] != "--require-stable-width"))
            throw new ArgumentException("PID, HWND, output JSON, optional --require-stable-width.");
        var requireStableWidth = args.Length == 4;
        using var process = Process.GetProcessById(int.Parse(args[0]));
        var hwnd = new IntPtr(long.Parse(args[1]));
        NativeInput.RequireSyntheticPackage(process, hwnd);
        var root = AutomationElement.FromHandle(hwnd);
        var searchGroup = Find(root, "VaultSearchBox");
        var edit = searchGroup.FindFirst(TreeScope.Subtree, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Edit))
            ?? throw new InvalidOperationException("Search textbox missing.");
        var value = (ValuePattern)edit.GetCurrentPattern(ValuePattern.Pattern);
        var count = Find(root, "ItemCount");
        var scroll = (ScrollPattern)Find(root, "VaultList").GetCurrentPattern(ScrollPattern.Pattern);
        var trials = new List<object>();
        var completed = false;
        try
        {
            const string query = "entry 00042 user00042";
            foreach (var interval in new[] { 0, 20, 60 })
            {
                value.SetValue("");
                Wait(() => count.Current.Name == "10000 个条目" && scroll.Current.VerticallyScrollable);
                edit.SetFocus();
                Wait(() => edit.Current.HasKeyboardFocus);
                Thread.Sleep(500); // Blank baseline frames; outside the timed typing window.
                var blankBounds = edit.Current.BoundingRectangle;
                var keys = new List<object>();
                var startUtc = DateTimeOffset.UtcNow;
                var start = Stopwatch.GetTimestamp();
                for (var index = 0; index < query.Length; index++)
                {
                    if (index > 0 && interval > 0) Thread.Sleep(interval);
                    NativeInput.RequireForeground(process.Id);
                    var sentUtc = DateTimeOffset.UtcNow;
                    var sent = Stopwatch.GetTimestamp();
                    NativeInput.SendCharacter(query[index]);
                    keys.Add(new { index, character = query[index].ToString(), prefix = query[..(index + 1)],
                        sentUtc, monotonicTicks = sent, elapsedMs = Stopwatch.GetElapsedTime(start, sent).TotalMilliseconds });
                }
                Wait(() => value.Current.Value == query && count.Current.Name == "1 个条目" && !scroll.Current.VerticallyScrollable);
                var readyUtc = DateTimeOffset.UtcNow;
                var typedBounds = edit.Current.BoundingRectangle;
                var stableWidth = Math.Abs(blankBounds.Width - typedBounds.Width) < 0.5 && Math.Abs(blankBounds.X - typedBounds.X) < 0.5;
                if (requireStableWidth && !stableWidth) throw new InvalidOperationException("Typing resized or moved the search textbox.");
                trials.Add(new { requestedIntervalMs = interval, startUtc, startTicks = start, readyUtc,
                    readyElapsedMs = Stopwatch.GetElapsedTime(start).TotalMilliseconds, keys,
                    blankBounds = new { x = blankBounds.X, width = blankBounds.Width },
                    typedBounds = new { x = typedBounds.X, width = typedBounds.Width }, stableWidth });
                Thread.Sleep(800); // Settled final glyph/reference frames; not part of the readiness time.
            }
            value.SetValue("");
            Wait(() => count.Current.Name == "10000 个条目" && scroll.Current.VerticallyScrollable);
            scroll.SetScrollPercent(ScrollPattern.NoScroll, 100);
            Wait(() => scroll.Current.VerticalScrollPercent >= 99);
            var viewport = Find(root, "VaultList").Current.BoundingRectangle;
            Wait(() => root.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ListItem))
                .Cast<AutomationElement>().Any(item => item.Current.Name.StartsWith("Synthetic entry 09999", StringComparison.Ordinal)
                    && !item.Current.IsOffscreen && item.Current.BoundingRectangle.Height > 0
                    && viewport.Contains(item.Current.BoundingRectangle)));
            completed = true;
        }
        finally
        {
            File.WriteAllText(args[2], JsonSerializer.Serialize(new {
                completed, targetPid = process.Id, targetHwnd = hwnd.ToInt64(), stopwatchFrequency = Stopwatch.Frequency,
                scope = "Public synthetic metadata only. UTC/monotonic SendInput markers for matching actual captured frames. NOT physical keyboard/IME latency. Recorder/GPU capture add overhead; these three trials are diagnostic, not production CPU comparisons.",
                searchBounds = new { x = edit.Current.BoundingRectangle.X, y = edit.Current.BoundingRectangle.Y,
                    width = edit.Current.BoundingRectangle.Width, height = edit.Current.BoundingRectangle.Height }, trials
            }, new JsonSerializerOptions { WriteIndented = true }));
        }
        Console.WriteLine("PASS: three recorded Unicode typing trials, exact text/count/layout and per-key timestamps.");
        return 0;
    }

    private static AutomationElement Find(AutomationElement root, string id) =>
        root.FindFirst(TreeScope.Subtree, new PropertyCondition(AutomationElement.AutomationIdProperty, id))
        ?? throw new InvalidOperationException("Missing " + id);

    private static void Wait(Func<bool> ready)
    {
        var watch = Stopwatch.StartNew();
        do { if (ready()) return; Thread.Sleep(2); } while (watch.Elapsed < TimeSpan.FromSeconds(20));
        throw new TimeoutException("Synthetic text/count/layout did not settle.");
    }
}
