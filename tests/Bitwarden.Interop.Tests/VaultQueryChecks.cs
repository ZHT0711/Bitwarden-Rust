using Bitwarden_WinUI.Interop;
using System.Diagnostics;
using System.Globalization;

internal static class VaultQueryChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var a = new VaultItem("a", "Alpha mail", "alice@example.invalid", 1, true, "folder", null, false, false, true, false,
            "2026-11-23T10:00:00+02:00");
        var b = a with { Id = "b", Name = "beta mail", Subtitle = "bob@example.invalid", Favorite = false, Reprompt = false,
            FolderId = null, RevisionDate = "2026-11-23T08:30:00Z" };
        var c = a with { Id = "c", Name = "Alpha bank", Subtitle = "customer", RevisionDate = "invalid" };
        VaultItem[] source = [b, c, a];
        string[] Ids(IEnumerable<VaultItem> items) => items.Select(item => item.Id).ToArray();
        check(Ids(VaultQuery.Apply(source, "vault", "*", "ALPHA alice")).SequenceEqual(["a"]), "metadata search ANDs words across name and subtitle");
        check(Ids(VaultQuery.Apply(source, "vault", "*", "alpha bank")).SequenceEqual(["c"]), "metadata search requires every term");
        check(Ids(VaultQuery.Apply(source, "vault", "*", "  alpha\t\nalice  ")).SequenceEqual(["a"]), "metadata search splits Unicode whitespace");
        check(Ids(VaultQuery.Apply(source, "vault", "*", "\"alpha mail\" alice")).SequenceEqual(["a"]), "metadata search combines a quoted phrase and another term");
        check(VaultQuery.Apply(source, "vault", "*", "\"mail alpha\"").Length == 0, "quoted metadata search preserves phrase order");
        check(Ids(VaultQuery.Apply(source, "vault", "*", "\"alpha mail")).SequenceEqual(["a"]), "unfinished search quote treats the remainder as a phrase");
        check(VaultQuery.Apply(source, "vault", "*", "\"\" \t").Length == 3, "empty search phrases are ignored");
        check(Ids(VaultQuery.Apply(source, "favorites", "folder", "alpha alice")).SequenceEqual(["a"]), "metadata words retain category and folder constraints");
        check(VaultQuery.Apply(source, "notes", "*", "alpha").Length == 0, "metadata words do not bypass type constraints");
        check(VaultQuery.Apply(source, "vault", "*", "password:secret").Length == 0, "metadata search does not interpret secret-field syntax");
        check(VaultQuery.Apply(source, "vault", "*", "alpha alice").Single().Reprompt, "metadata search preserves master-password reprompt flags");
        check(ReferenceEquals(VaultQuery.Apply(source, "vault", "*", "bob").Single(), b), "metadata filtering reuses the existing item projection");
        check(Ids(VaultQuery.Sort(source, VaultSort.NameAscending)).SequenceEqual(["c", "a", "b"]), "name ascending sorts case-insensitively");
        check(Ids(VaultQuery.Sort(source, VaultSort.NameDescending)).SequenceEqual(["b", "a", "c"]), "name descending sorts case-insensitively");
        check(Ids(VaultQuery.Sort(source, VaultSort.RevisionNewest)).SequenceEqual(["b", "a", "c"]), "newest revisions compare instants and put invalid dates last");
        check(Ids(VaultQuery.Sort(source, VaultSort.RevisionOldest)).SequenceEqual(["a", "b", "c"]), "oldest revisions compare instants and put invalid dates last");
        var duplicates = new[] { a with { Id = "z" }, a with { Id = "d", Name = "ALPHA MAIL" } };
        foreach (var sort in Enum.GetValues<VaultSort>())
            check(Ids(VaultQuery.Sort(duplicates, sort)).SequenceEqual(["d", "z"]), $"sort {sort} uses a stable ordinal ID tie-break");
        check(Ids(source).SequenceEqual(["b", "c", "a"]), "sorting leaves the original overview order unchanged");
        check(VaultQuery.Sort([], VaultSort.NameAscending).Length == 0, "sorting an empty vault returns no items");
        check(VaultQuery.Sort([a with { Name = "" }], VaultSort.NameAscending).Single().DisplayName == "（未命名条目）", "sorting supports unnamed items");
        check(Ids(VaultQuery.Sort(source, (VaultSort)999)).SequenceEqual(Ids(VaultQuery.Sort(source, VaultSort.NameAscending))), "unsupported sort values fall back to ascending names");
        var culture = CultureInfo.CurrentCulture;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("tr-TR");
            check(Ids(VaultQuery.Sort(source, VaultSort.RevisionNewest)).SequenceEqual(["b", "a", "c"]), "revision ordering is independent of Windows culture");
        }
        finally { CultureInfo.CurrentCulture = culture; }
        check(VaultQuery.SameProjection(source, [b, c, a]), "equivalent filtering retains the exact projected objects and order");
        check(!VaultQuery.SameProjection(source, [a, b, c]), "reordering is distinguished from a no-op filter");
        check(!VaultQuery.SameProjection(source, [b with { }, c, a]), "a new overview record cannot be mistaken for a retained detail authorization");
        check(VaultQuery.RetainSelection([], source).Length == 0, "empty selection does not allocate a vault-wide selection index");
        check(ReferenceEquals(VaultQuery.RetainSelection([a], [a, b]).Single(), a), "sorting retains the selected object without reauthorizing details");
        check(VaultQuery.RetainSelection([a], [a with { }]).Length == 0, "new record projections revoke the old selection even when their fields match");
        check(Ids(VaultQuery.RetainSelection([a, b, c], [c, a])).SequenceEqual(["a", "c"]), "bulk filtering retains only visible objects in the previous selection order");
        check(VaultQuery.RetainSelection([a], [b]).Length == 0, "filtering out the selected object removes the selection");
        var invisible = source.Concat([a with { Id = "trash", Deleted = true }, b with { Id = "archive", Archived = true }]);
        check(VaultQuery.Apply(VaultQuery.Sort(invisible, VaultSort.RevisionNewest), "vault", "*", "mail").Length == 2,
            "sorted search excludes trash and archived items");
        var large = Enumerable.Range(0, 10_000).Select(index => a with
        {
            Id = index.ToString(CultureInfo.InvariantCulture), Name = "Synthetic metadata",
            Subtitle = $"user{index % 100}@example.invalid",
        }).ToArray();
        for (var warmup = 0; warmup < 10; warmup++) _ = VaultQuery.Apply(large, "vault", "*", "synthetic user42");
        var allocatedBefore = GC.GetAllocatedBytesForCurrentThread();
        var largeResult = VaultQuery.Apply(large, "vault", "*", "synthetic user42");
        var allocated = GC.GetAllocatedBytesForCurrentThread() - allocatedBefore;
        check(largeResult.Length == 100 && ReferenceEquals(largeResult[0], large[42]), "10000-item AND search keeps exact metadata objects and source order");
        check(allocated < 32_768, "10000-item metadata search avoids per-row closure/delegate allocations");
    }

    internal static void Measure()
    {
        foreach (var size in new[] { 100, 1_000, 10_000 })
        {
            var source = Enumerable.Range(0, size).Select(index => new VaultItem(index.ToString(CultureInfo.InvariantCulture),
                $"Synthetic item {index:D5}", $"user{index % 100}@example.invalid", 1, false, null, null, false, false, false, false)).ToArray();
            var watch = Stopwatch.StartNew();
            var ordered = VaultQuery.Sort(source, VaultSort.NameAscending);
            watch.Stop();
            var sortMilliseconds = watch.Elapsed.TotalMilliseconds;
            for (var warmup = 0; warmup < 10; warmup++) _ = VaultQuery.Apply(ordered, "vault", "*", "synthetic user42");
            var samples = new double[100];
            var allocatedBefore = GC.GetAllocatedBytesForCurrentThread();
            for (var index = 0; index < samples.Length; index++)
            {
                watch.Restart();
                var result = VaultQuery.Apply(ordered, "vault", "*", "synthetic user42");
                watch.Stop();
                if (result.Length != size / 100) throw new InvalidOperationException("Synthetic search count changed.");
                samples[index] = watch.Elapsed.TotalMilliseconds;
            }
            var allocatedPerQuery = (GC.GetAllocatedBytesForCurrentThread() - allocatedBefore) / samples.Length;
            Array.Sort(samples);
            Console.WriteLine(FormattableString.Invariant($"metadata-search items={size} initial-sort-ms={sortMilliseconds:F4} median-ms={samples[50]:F4} p95-ms={samples[94]:F4} allocated-bytes-per-query={allocatedPerQuery}"));
        }
    }
}
