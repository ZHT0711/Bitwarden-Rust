using System.Diagnostics;
using System.IO;
using System.Text.Json;
using System.Windows.Automation;
using System.Windows;

internal static class Program
{
    private static int Main(string[] args)
    {
        if (args.Length != 6) throw new ArgumentException("Expected PID, HWND, item count, last item name, output JSON, idle seconds.");
        var pid = int.Parse(args[0]);
        var hwnd = new IntPtr(long.Parse(args[1]));
        var count = int.Parse(args[2]);
        var lastName = args[3];
        var output = Path.GetFullPath(args[4]);
        var idleSeconds = int.Parse(args[5]);
        if (count != 10_000 || idleSeconds is < 0 or > 600) throw new ArgumentOutOfRangeException(nameof(args));
        using var process = Process.GetProcessById(pid);
        NativeInput.RequireSyntheticPackage(process, hwnd);
        var root = AutomationElement.FromHandle(hwnd);
        var searchGroup = Find(root, "VaultSearchBox");
        var search = searchGroup.FindFirst(TreeScope.Subtree,
            new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Edit))
            ?? throw new InvalidOperationException("Missing search edit control.");
        var itemCount = Find(root, "ItemCount");
        var list = Find(root, "VaultList");
        var scroll = (ScrollPattern)list.GetCurrentPattern(ScrollPattern.Pattern);
        var value = (ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern);
        var samples = new List<object>();
        var memory = new List<object>();
        var completed = false;
        string? error = null;
        try
        {
            Capture("loaded");
            Wait(() => itemCount.Current.Name == $"{count} 个条目", "loaded count");
            const string query = "entry 00042 user00042";
            // Warm up client/UIA work separately. All text is public synthetic display metadata.
            SetAndWait(query, 1);
            SetAndWait("", count);
            foreach (var interval in new[] { 0, 20, 60 })
            {
                for (var iteration = 0; iteration < 5; iteration++)
                {
                    SetAndWait("", count);
                    search.SetFocus();
                    Wait(() => search.Current.HasKeyboardFocus, "search focus");
                    NativeInput.RequireForeground(pid);
                    var cpuBefore = process.TotalProcessorTime.TotalMilliseconds;
                    var started = Stopwatch.GetTimestamp();
                    long lastKey = started;
                    for (var index = 0; index < query.Length; index++)
                    {
                        if (index > 0 && interval > 0) Thread.Sleep(interval);
                        NativeInput.RequireForeground(pid); // Refuse input if the operator switches windows.
                        lastKey = Stopwatch.GetTimestamp();
                        NativeInput.SendCharacter(query[index]);
                    }
                    var injectionMs = Stopwatch.GetElapsedTime(started).TotalMilliseconds;
                    Wait(() => value.Current.Value == query && itemCount.Current.Name == "1 个条目" &&
                        !scroll.Current.VerticallyScrollable, "typed text, exact query result and one-row scroll layout");
                    samples.Add(new
                    {
                        action = "unicode-input-to-uia-ready", keyIntervalMs = interval, iteration,
                        injectedCharacters = query.Length, injectionMs,
                        lastKeyToReadyMs = Stopwatch.GetElapsedTime(lastKey).TotalMilliseconds,
                        totalMs = Stopwatch.GetElapsedTime(started).TotalMilliseconds,
                        applicationCpuMs = process.TotalProcessorTime.TotalMilliseconds - cpuBefore,
                    });
                }
                Capture($"after-input-{interval}ms");
            }
            SetAndWait("", count);
            var watch = Stopwatch.StartNew();
            scroll.SetScrollPercent(ScrollPattern.NoScroll, 100);
            AutomationElement? last = null;
            Wait(() =>
            {
                last = list.FindFirst(TreeScope.Children, new PropertyCondition(AutomationElement.NameProperty, lastName));
                return last is not null && !last.Current.IsOffscreen && Inside(last.Current.BoundingRectangle, list.Current.BoundingRectangle);
            }, "last actual row entirely inside virtualized viewport");
            samples.Add(new { action = "scroll-bottom-to-uia-ready", elapsedMs = watch.Elapsed.TotalMilliseconds });
            Capture("after-scroll-bottom");
            scroll.SetScrollPercent(ScrollPattern.NoScroll, 0);
            // No UIA queries or input during idle sampling. The target app, not this probe, is measured.
            var idleWatch = Stopwatch.StartNew();
            var idleCpu = process.TotalProcessorTime.TotalMilliseconds;
            for (var second = 0; second < idleSeconds; second++)
            {
                Thread.Sleep(1000);
                Capture($"idle-{second + 1}s");
            }
            var idleMs = idleWatch.Elapsed.TotalMilliseconds;
            var cpuMs = process.TotalProcessorTime.TotalMilliseconds - idleCpu;
            samples.Add(new { action = "unlocked-idle", elapsedMs = idleMs, applicationCpuMs = cpuMs,
                cpuPercentOfOneCore = idleMs > 0 ? cpuMs / idleMs * 100 : 0 });
            completed = true;
        }
        catch (Exception exception) { error = exception.Message; throw; }
        finally
        {
            Directory.CreateDirectory(Path.GetDirectoryName(output)!);
            File.WriteAllText(output, JsonSerializer.Serialize(new
            {
                completed, error, targetPid = pid, targetHwnd = hwnd.ToInt64(), items = count,
                timingScope = "Native Unicode SendInput to exact textbox/ItemCount/scroll-layout UIA readiness in a persistent probe. Includes UIA provider/polling and OS scheduling; NOT physical keyboard/IME latency or rendered frame time.",
                memoryScope = "Actual application process private bytes/working set and cumulative OS peaks; idle has no UIA polling. Not an exact private peak or proof of long-term stability.",
                samples, memory,
            }, new JsonSerializerOptions { WriteIndented = true }));
        }
        Console.WriteLine($"PASS: 10000 encrypted items; {samples.Count} measured input/scroll/idle observations. {output}");
        return 0;

        void SetAndWait(string text, int expected)
        {
            value.SetValue(text);
            Wait(() => value.Current.Value == text && itemCount.Current.Name == $"{expected} 个条目" &&
                scroll.Current.VerticallyScrollable == (expected > 1), "search reset/query and scroll layout");
        }
        void Capture(string stage)
        {
            process.Refresh();
            memory.Add(new { stage, timestamp = DateTimeOffset.UtcNow, privateBytes = process.PrivateMemorySize64,
                workingSetBytes = process.WorkingSet64, peakWorkingSetBytes = process.PeakWorkingSet64,
                cpuMs = process.TotalProcessorTime.TotalMilliseconds });
        }
    }

    private static AutomationElement Find(AutomationElement root, string id) =>
        root.FindFirst(TreeScope.Subtree, new PropertyCondition(AutomationElement.AutomationIdProperty, id))
        ?? throw new InvalidOperationException($"Missing UIA control: {id}");

    private static bool Inside(Rect child, Rect parent) => child.Width > 0 && child.Height > 0 &&
        child.Left >= parent.Left - 1 && child.Top >= parent.Top - 1 &&
        child.Right <= parent.Right + 1 && child.Bottom <= parent.Bottom + 1;

    private static void Wait(Func<bool> ready, string description)
    {
        var watch = Stopwatch.StartNew();
        do
        {
            if (ready()) return;
            Thread.Sleep(2);
        } while (watch.Elapsed < TimeSpan.FromSeconds(20));
        throw new TimeoutException("UI did not settle: " + description);
    }
}
