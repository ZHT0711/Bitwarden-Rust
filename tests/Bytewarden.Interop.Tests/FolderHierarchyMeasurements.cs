using Bytewarden.WinUI.Interop;
using System.Diagnostics;
using System.Globalization;

internal static class FolderHierarchyMeasurements
{
    internal static void Run()
    {
        foreach (var depth in new[] { 512, 2_048 })
        {
            var tail = string.Concat(Enumerable.Repeat("Missing/", depth)) + "Leaf";
            Measure($"orphan-{depth}", [new("root", "Anchor"), new("leaf", tail)], [], 2);
            Measure($"joined-{depth}", [new("root", "Anchor"), new("leaf", "Anchor/" + tail)], [], 1);
        }
        Measure("flat-10000", Enumerable.Range(0, 10_000).Select(index => new VaultFolder($"f{index}", $"Folder {index:D5}")).ToArray(), [], 10_000);
        Measure("nested-1000", Enumerable.Range(0, 1_000).Select(index => new VaultFolder($"f{index}", string.Concat(Enumerable.Repeat("X/", index)) + "X")).ToArray(), [], 1);
        var branches = Enumerable.Range(0, 100).Select(index => new VaultFolder($"r{index}", $"Group {index:D2}"))
            .Concat(Enumerable.Range(0, 9_900).Select(index => new VaultFolder($"f{index}", $"Group {index % 100:D2}/Child {index:D4}"))).ToArray();
        var items = Enumerable.Range(0, 10_000).Select(index => new VaultItem($"i{index}", "Synthetic list metadata", "", 1,
            false, $"f{index % 9_900}", null, false, false, false, false)).ToArray();
        Measure("branching-10000", branches, items, 100);
    }

    private static void Measure(string label, VaultFolder[] folders, VaultItem[] items, int expectedRoots)
    {
        for (var warmup = 0; warmup < 3; warmup++) Validate(FolderHierarchy.Build(folders, items));
        var times = new double[25];
        var allocations = new long[25];
        var watch = new Stopwatch();
        for (var sample = 0; sample < times.Length; sample++)
        {
            var before = GC.GetAllocatedBytesForCurrentThread();
            watch.Restart();
            var tree = FolderHierarchy.Build(folders, items);
            watch.Stop();
            allocations[sample] = GC.GetAllocatedBytesForCurrentThread() - before;
            times[sample] = watch.Elapsed.TotalMilliseconds;
            Validate(tree);
        }
        Array.Sort(times);
        Array.Sort(allocations);
        Console.WriteLine(FormattableString.Invariant($"folder-index case={label} folders={folders.Length} items={items.Length} input-chars={folders.Sum(folder => folder.Name.Length)} median-ms={times[12]:F4} p95-ms={times[23]:F4} allocated-median-bytes={allocations[12]}"));

        void Validate(FolderHierarchy tree)
        {
            if (tree.All.Length != folders.Length || tree.Roots.Length != expectedRoots || tree.Roots.Sum(node => node.ItemCount) != items.Length)
                throw new InvalidOperationException($"Synthetic folder benchmark shape changed: {label}");
        }
    }
}
