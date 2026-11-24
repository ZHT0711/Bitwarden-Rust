using Bitwarden_WinUI.Interop;
using System.Globalization;

internal static class FolderHierarchyChecks
{
    internal static void Run(Action<bool, string> check)
    {
        VaultFolder[] folders = [new("grand", "A/Child/Leaf"), new("child", "A/Child"), new("root", "A"),
            new("orphan", "Missing/Joined/Leaf"), new("empty", ""), new("none", "   "), new("", "Not a real folder")];
        var sample = new VaultItem("item", "Synthetic metadata", "", 1, false, "root", null, false, false, false, false);
        VaultItem[] items = [sample, sample with { Id = "child-item", FolderId = "child" },
            sample with { Id = "grand-item", FolderId = "grand", Reprompt = true },
            sample with { Id = "archived", FolderId = "child", Archived = true },
            sample with { Id = "deleted", FolderId = "child", Deleted = true },
            sample with { Id = "restricted", FolderId = "grand", Restricted = true },
            sample with { Id = "orphan-item", FolderId = "orphan" },
            sample with { Id = "unknown", FolderId = "unknown" }, sample with { Id = "unfiled", FolderId = null }];
        var tree = FolderHierarchy.Build(folders, items);
        var root = tree.Find("root")!;
        var child = tree.Find("child")!;
        var grand = tree.Find("grand")!;
        check(tree.Roots.Length == 4 && tree.All.Length == 6, "folders show only real roots and exclude the no-folder sentinel");
        check(root.Parent is null && root.Children.Single() == child && child.Children.Single() == grand,
            "folders connect a three-level hierarchy independently of source order");
        check(child.DisplayName == "Child" && grand.DisplayName == "Leaf", "folders strip existing parent paths only for display");
        check(child.Name == "A/Child" && folders[0].Name == "A/Child/Leaf", "folder nesting never changes the full stored names");
        check(root.ItemCount == 5 && root.DirectItemCount == 1, "folder root counts include descendants without changing direct membership");
        check(child.ItemCount == 4 && child.DirectItemCount == 2 && grand.ItemCount == 2,
            "folder subtree counts accumulate once across multiple levels");
        check(root.ItemCount == 5 && tree.Find("unknown") is null, "folder counts exclude trash, missing IDs and unfiled items");
        check(grand.ItemCount == 2 && items[2].Reprompt && items[5].Restricted, "folder counting retains restricted and reprompt metadata without opening details");
        check(tree.Find("orphan") is { Parent: null, DisplayName: "Missing/Joined/Leaf", ItemCount: 1 },
            "folder missing ancestors remain one real root with a joined display name");
        check(tree.Find("empty")!.DisplayName == "—" && tree.Find("none")!.DisplayName == "—", "unnamed folders have an accessible placeholder");
        check(tree.Trail("grand").Select(node => node.Id).SequenceEqual(["root", "child", "grand"]), "folder trail contains real ancestors and the current folder");
        check(tree.Trail("orphan").Length == 1 && tree.Trail("unknown").Length == 0, "folder trails never invent missing ancestors");
        check(tree.All.Select(node => node.Id).Distinct().Count() == tree.All.Length, "folder preorder visits each ID exactly once");
        check(VaultQuery.Apply(items, "vault", "root", "").Single().Id == "item", "parent item filtering does not include descendant items");
        check(VaultQuery.Apply(items, "vault", "child", "").Single().Id == "child-item", "child item filtering still excludes archive and trash");
        var row = root.Row(true);
        check(row.NestedFolders == 1 && row.ItemCountLabel == "5 个条目（含子文件夹），1 个子文件夹", "folder table labels explain aggregated counts and direct child count");
        check(grand.Row(true).ItemCountLabel == "2 个条目", "leaf folder counts do not claim nonexistent children");
        check(child.Row(true).DisplayName == "Child" && child.Row(true, fullName: true).DisplayName == "A/Child", "folder full-name search rows retain their path context");
        check(child.Row(false) is { CanEdit: false, Id: "child", Name: "A/Child" }, "read-only folder rows retain navigation IDs but disable writes");
        check(child.Row(true).ToString() == "A/Child" && !child.Row(true).ToString().Contains("Id ="), "accessible folder names avoid record dumps");
        check(child.Row(true).OpenLabel == "打开文件夹“A/Child”" && child.Row(true).RenameLabel.Contains("A/Child") &&
            child.Row(true).DeleteLabel.Contains("A/Child"), "folder row actions have distinct accessible full-path labels");
        check(new FolderCrumb("child", "Child").ToString() == "Child", "accessible breadcrumb text avoids record dumps");
        check(child.Row(true).OpenAutomationId == "FolderOpen_child" &&
            (child.Row(true) with { Name = "Renamed/path" }).OpenAutomationId == "FolderOpen_child" &&
            grand.Row(true).OpenAutomationId != child.Row(true).OpenAutomationId,
            "folder open automation IDs are unique server IDs independent of full names");

        var choice = new FolderChoice("same-id", " /Original name/ ");
        check(choice.Name == " /Original name/ " && choice.DisplayName == choice.Name, "folder choices preserve full stored names including whitespace and delimiters");
        check(choice.ToString() == choice.Name && !choice.ToString().Contains("Id ="), "data-bound folder choices expose labels rather than record dumps");
        check((choice with { Name = "Changed" }).Id == choice.Id, "folder choice identity does not depend on display text");
        check(new FolderChoice("empty", "  ") is { Name: "  ", DisplayName: "—" }, "unnamed folder choices change only their visible placeholder");
        var missing = FolderHierarchy.Build([new("top", "3"), new("joined", "3/3.1/3.1.1"), new("leaf", "3/3.1/3.1.1/Leaf")], []);
        check(missing.Find("joined")!.Parent == missing.Find("top") && missing.Find("joined")!.DisplayName == "3.1/3.1.1",
            "folders reproduce the official missing-intermediate-parent example");
        check(missing.Find("leaf")!.Parent == missing.Find("joined") && missing.Find("leaf")!.DisplayName == "Leaf",
            "joined orphan nodes can themselves contain real children");
        var boundary = FolderHierarchy.Build([new("top", "A"), new("trim", "/A/B/"), new("repeat", "A//Repeated"),
            new("case", "a/Other"), new("prefix", "AB/Other"), new("slashes", "///")], []);
        check(boundary.Find("trim")!.Parent == boundary.Find("top") && boundary.Find("trim")!.DisplayName == "B" && boundary.Find("trim")!.Name == "/A/B/",
            "folder display ignores boundary slashes without rewriting stored names");
        check(boundary.Find("repeat")!.Parent == boundary.Find("top") && boundary.Find("repeat")!.DisplayName == "/Repeated",
            "repeated folder delimiters remain a joined display name, not an empty virtual folder");
        check(boundary.Find("case")!.Parent is null && boundary.Find("prefix")!.Parent is null, "folder nesting matches case-sensitive path segments rather than text prefixes");
        check(boundary.Find("slashes")!.DisplayName == "—" && boundary.Find("slashes")!.Children.Length == 0, "slash-only folder names do not become virtual parents");
        var duplicates = FolderHierarchy.Build([new("p2", "A"), new("p1", "A"), new("c", "A/C")], []);
        check(duplicates.Roots.Length == 2 && duplicates.Find("c")!.Parent!.Id == "p1", "same-name folder IDs are preserved with a deterministic parent tie-break");
        check(FolderHierarchy.Build([new("id", "A"), new("id", "Ignored duplicate ID")], []).All.Length == 1, "a duplicate folder ID cannot produce duplicate navigation entries");
        check(FolderHierarchy.Build([], items).Roots.Length == 0, "empty folder snapshots ignore all stale item references");
        var culture = CultureInfo.CurrentCulture;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("tr-TR");
            check(FolderHierarchy.Build(folders.Reverse(), items).All.Select(node => node.Id).SequenceEqual(tree.All.Select(node => node.Id)),
                "folder ordering is deterministic across input order and Windows culture");
        }
        finally { CultureInfo.CurrentCulture = culture; }
        var deep = FolderHierarchy.Build(Enumerable.Range(0, 1_000).Select(index => new VaultFolder($"d{index}", string.Concat(Enumerable.Repeat("X/", index)) + "X")), []);
        check(deep.Roots.Length == 1 && deep.All.Length == 1_000 && deep.Trail("d999").Length == 1_000,
            "folder nesting and breadcrumbs handle 1000 levels without recursive stack growth");
        var navigation = deep.Navigation("d999");
        check(navigation.Visible.Length == 9 && navigation.Earlier.Length == 992 && navigation.Visible[0].Id is null,
            "deep folder navigation bounds breadcrumb visuals without inserting a virtual folder");
        check(navigation.Earlier.Select(row => row.Id).Concat(navigation.Visible.Skip(1).Select(crumb => crumb.Id!))
            .SequenceEqual(deep.Trail("d999").Select(node => node.Id)), "early ancestor menu and visible trail retain every real ID in order exactly once");
        check(navigation.Earlier.All(row => !row.CanEdit) && navigation.Earlier[0].Name == "X" && navigation.Visible[^1].Id == "d999",
            "ancestor menu is navigation-only and retains full names and current folder identity");
        check(tree.Navigation("grand").Visible.Length == 4 && tree.Navigation("grand").Earlier.Length == 0 &&
            tree.Navigation("unknown").Visible.Length == 0, "short and missing trails do not acquire an unnecessary ancestor menu");
        var unicodePath = new string('a', 63) + "😀" + new string('b', 300) + "😀" + new string('c', 93);
        var display = FolderDisplay.Compact(unicodePath);
        check(display.Length <= 160 && display.Contains('…') && display.EnumerateRunes().All(rune => rune.Value != 0xfffd),
            "compact folder labels bound text layout without splitting UTF16 surrogate pairs");
        var longChoice = new FolderChoice("unicode-id", unicodePath);
        check(longChoice.DisplayText == display && longChoice.Name == unicodePath && longChoice.ToString() == unicodePath,
            "bounded choice labels never change full accessible names or storage IDs");
        check((row with { DisplayName = unicodePath }).DisplayText == display && new FolderCrumb("id", unicodePath).DisplayText == display,
            "rows and breadcrumbs share display-only abbreviation without path normalization");
        check(FolderHierarchy.Build([new("root", "Different account")], []).Find("root")!.Name == "Different account" && root.Name == "A",
            "folder trees are independent snapshots with no cross-account static cache");

        VaultFolder[] prefixes = [new("a", "A"), new("punct", "A!"), new("space", "A ."), new("child", "A/B"),
            new("prefix", "A/BC"), new("leaf", "A/BC/D"), new("sibling", "A/C"), new("accent", "A/é"),
            new("unicode", "A/😀/Leaf"), new("repeated", "A//B"), new("duplicate", "/A/BC/"), new("cousin", "AB/Leaf")];
        check(MatchesOracle(prefixes, []), "folder prefix stack handles punctuation, Unicode, duplicate trimmed paths and segment boundaries");
        var prefixTree = FolderHierarchy.Build(prefixes, []);
        check(prefixTree.Find("child")!.Parent!.Id == "a" && prefixTree.Find("prefix")!.Parent!.Id == "a" &&
            prefixTree.Find("leaf")!.Parent!.Id == "duplicate" && prefixTree.Find("cousin")!.Parent is null,
            "character-prefix candidates never shadow a real slash-delimited ancestor");

        var random = new Random(731042);
        var vocabulary = new[] { "A", "AB", "A!", "a", "B", " ", "", "é", "😀", "Child", "Childish" };
        var matches = true;
        for (var snapshot = 0; snapshot < 100; snapshot++)
        {
            var generated = Enumerable.Range(0, 120).Select(index => new VaultFolder($"f{index}",
                string.Join('/', Enumerable.Range(0, random.Next(1, 7)).Select(_ => vocabulary[random.Next(vocabulary.Length)])))).ToArray();
            var metadata = Enumerable.Range(0, 240).Select(index => sample with { Id = $"i{index}",
                FolderId = random.Next(5) == 0 ? "unknown" : $"f{random.Next(generated.Length)}",
                Deleted = random.Next(5) == 0, Archived = random.Next(4) == 0 }).ToArray();
            matches &= MatchesOracle(generated, metadata) && MatchesOracle(generated.Reverse().ToArray(), metadata);
        }
        check(matches, "100 seeded folder snapshots and reversed inputs match an independent all-candidates oracle");

        var tail = string.Concat(Enumerable.Repeat("Missing/", 8_192)) + "Leaf";
        var longFolders = new VaultFolder[] { new("anchor", "Anchor"), new("orphan", tail), new("joined", "Anchor/" + tail) };
        FolderHierarchy.Build(longFolders, []); // Warm up before allocation evidence; inputs are outside the measured scope.
        var before = GC.GetAllocatedBytesForCurrentThread();
        var longTree = FolderHierarchy.Build(longFolders, []);
        var allocated = GC.GetAllocatedBytesForCurrentThread() - before;
        check(longTree.Roots.Length == 2 && longTree.All.Length == 3 && longTree.Find("joined")!.Parent!.Id == "anchor" &&
            longTree.Find("joined")!.DisplayName == tail && longTree.Find("orphan")!.DisplayName == tail,
            "8192 missing segments keep joined display paths and real IDs without virtual nodes");
        check(allocated < 1_000_000, "long missing folder paths allocate under 1 MB, not quadratic temporary prefixes");

        var largeFolders = Enumerable.Range(0, 100).Select(index => new VaultFolder($"r{index}", $"Group {index:D2}"))
            .Concat(Enumerable.Range(0, 9_900).Select(index => new VaultFolder($"f{index}", $"Group {index % 100:D2}/Child {index:D4}"))).ToArray();
        var largeItems = Enumerable.Range(0, 10_000).Select(index => sample with { Id = $"i{index}", FolderId = $"f{index % 9_900}" }).ToArray();
        var large = FolderHierarchy.Build(largeFolders, largeItems);
        check(large.Roots.Length == 100 && large.All.Length == 10_000 && large.Roots.Sum(node => node.ItemCount) == 10_000 &&
            large.Roots.All(node => node.Children.Length == 99 && node.ItemCount == 100),
            "10000-folder branching index counts all metadata once and keeps 99 real children per root");
    }

    // Deliberately simple O(n²) reference: inspect every possible real parent, rather than
    // reproducing production's sorting/stack algorithm or its old delimiter-slicing loop.
    private static bool MatchesOracle(VaultFolder[] folders, VaultItem[] items)
    {
        var ordered = folders.Where(folder => !string.IsNullOrEmpty(folder.Id)).DistinctBy(folder => folder.Id)
            .OrderBy(folder => folder.Name, StringComparer.OrdinalIgnoreCase)
            .ThenBy(folder => folder.Name, StringComparer.Ordinal).ThenBy(folder => folder.Id, StringComparer.Ordinal).ToArray();
        var paths = ordered.ToDictionary(folder => folder.Id, folder => folder.Name.Trim('/'));
        var parents = ordered.ToDictionary(folder => folder.Id, folder => ordered
            .Where(candidate => paths[candidate.Id].Length > 0 && paths[folder.Id].StartsWith(paths[candidate.Id] + "/", StringComparison.Ordinal))
            .OrderByDescending(candidate => paths[candidate.Id].Length).FirstOrDefault()?.Id);
        var counts = ordered.ToDictionary(folder => folder.Id, _ => 0);
        var direct = ordered.ToDictionary(folder => folder.Id, _ => 0);
        foreach (var item in items.Where(item => !item.Deleted))
            if (item.FolderId is { } id && parents.ContainsKey(id))
            {
                direct[id]++;
                for (string? ancestor = id; ancestor is not null; ancestor = parents[ancestor]) counts[ancestor]++;
            }
        var tree = FolderHierarchy.Build(folders, items);
        var expectedOrder = new List<string>();
        var pending = new Stack<string>(ordered.Where(folder => parents[folder.Id] is null).Select(folder => folder.Id).Reverse());
        while (pending.TryPop(out var id))
        {
            expectedOrder.Add(id);
            foreach (var child in ordered.Where(folder => parents[folder.Id] == id).Reverse()) pending.Push(child.Id);
        }
        return tree.All.Select(node => node.Id).SequenceEqual(expectedOrder) && tree.All.All(node =>
        {
            var parent = parents[node.Id];
            var display = parent is null ? paths[node.Id] : paths[node.Id][(paths[parent].Length + 1)..];
            return node.Parent?.Id == parent && node.Path == paths[node.Id] &&
                node.DisplayName == (string.IsNullOrWhiteSpace(display) ? "—" : display) &&
                node.ItemCount == counts[node.Id] && node.DirectItemCount == direct[node.Id] &&
                node.Children.Select(child => child.Id).SequenceEqual(ordered.Where(folder => parents[folder.Id] == node.Id).Select(folder => folder.Id));
        });
    }
}
