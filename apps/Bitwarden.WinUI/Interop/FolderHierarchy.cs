namespace Bitwarden_WinUI.Interop;

/// <summary>Display-only nesting. Server folders remain independent IDs with full encrypted names.</summary>
internal sealed class FolderHierarchy
{
    internal sealed class Node(VaultFolder folder, string path)
    {
        internal string Id => folder.Id;
        internal string Name => folder.Name;
        internal string Path { get; } = path;
        internal string DisplayName { get; set; } = string.IsNullOrWhiteSpace(path) ? "—" : path;
        internal Node? Parent { get; set; }
        internal Node[] Children { get; set; } = [];
        internal int DirectItemCount { get; set; }
        internal int ItemCount { get; set; }
        internal FolderRow Row(bool canEdit, bool fullName = false) => new(Id, Name,
            fullName && !string.IsNullOrWhiteSpace(Name) ? Name : DisplayName,
            ItemCount, DirectItemCount, Children.Length, canEdit);
    }

    private readonly Dictionary<string, Node> _byId;
    internal Node[] Roots { get; }
    internal Node[] All { get; }

    private FolderHierarchy(Node[] roots, Node[] all, Dictionary<string, Node> byId)
    {
        Roots = roots;
        All = all;
        _byId = byId;
    }

    internal Node? Find(string? id) => id is not null && _byId.TryGetValue(id, out var node) ? node : null;

    internal Node[] Trail(string id)
    {
        var result = new List<Node>();
        for (var node = Find(id); node is not null; node = node.Parent) result.Add(node);
        result.Reverse();
        return result.ToArray();
    }

    // Window the visuals, not the folder tree. Omitted entries remain real, navigable ancestor IDs.
    internal (FolderCrumb[] Visible, FolderRow[] Earlier) Navigation(string id, int maxVisible = 8)
    {
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(maxVisible);
        var trail = Trail(id);
        if (trail.Length == 0) return ([], []);
        var earlier = Math.Max(0, trail.Length - maxVisible);
        return ([new(null, "我的文件夹"), .. trail.Skip(earlier).Select(node => new FolderCrumb(node.Id, node.DisplayName))],
            trail.Take(earlier).Select(node => node.Row(canEdit: false)).ToArray());
    }

    internal static FolderHierarchy Build(IEnumerable<VaultFolder> folders, IEnumerable<VaultItem> items)
    {
        var nodes = folders.Where(folder => !string.IsNullOrEmpty(folder.Id)).DistinctBy(folder => folder.Id)
            .OrderBy(folder => folder.Name, StringComparer.OrdinalIgnoreCase)
            .ThenBy(folder => folder.Name, StringComparer.Ordinal)
            .ThenBy(folder => folder.Id, StringComparer.Ordinal)
            .Select(folder => new Node(folder, (folder.Name ?? string.Empty).Trim('/'))).ToArray();
        var byId = new Dictionary<string, Node>(StringComparer.Ordinal);
        var children = new Dictionary<Node, List<Node>>();
        foreach (var node in nodes) byId.Add(node.Id, node);

        // Ordinal order keeps character-prefix ranges contiguous. Each candidate is
        // pushed/popped once; missing paths do not allocate successively shorter slices.
        // A character prefix is not necessarily a folder ancestor (A/B vs A/BC); in that
        // case its real parent is the nearest valid ancestor of the longer path too.
        // OrderBy is stable: duplicate paths retain the original name/ID tie-break above.
        var prefixes = new Stack<Node>();
        Node? previous = null;
        foreach (var node in nodes.OrderBy(node => node.Path, StringComparer.Ordinal))
        {
            if (previous is not null && node.Path == previous.Path)
            {
                // Preserve separate IDs, but only the first path representative is a parent.
                node.Parent = previous.Parent;
            }
            else
            {
                while (prefixes.TryPeek(out var candidate) &&
                    !node.Path.StartsWith(candidate.Path, StringComparison.Ordinal))
                    prefixes.Pop();
                node.Parent = prefixes.TryPeek(out var prefix)
                    ? node.Path[prefix.Path.Length] == '/' ? prefix : prefix.Parent
                    : null;
                if (node.Path.Length > 0) prefixes.Push(node);
            }
            previous = node;
        }

        var roots = new List<Node>();
        foreach (var node in nodes)
        {
            // Only a REAL parent strips its path; joined missing segments stay visible.
            // Keep the UI's original ordering, stored full names and exact server IDs.
            if (node.Parent is { } parent)
            {
                node.DisplayName = node.Path[(parent.Path.Length + 1)..];
                if (string.IsNullOrWhiteSpace(node.DisplayName)) node.DisplayName = "—";
                if (!children.TryGetValue(parent, out var branch)) children[parent] = branch = [];
                branch.Add(node);
            }
            else roots.Add(node);
        }
        foreach (var (node, branch) in children) node.Children = branch.ToArray();

        // Iterative preorder and accumulation avoid a stack overflow on deeply nested names.
        var all = new List<Node>(nodes.Length);
        var pending = new Stack<Node>(roots.AsEnumerable().Reverse());
        while (pending.TryPop(out var node))
        {
            all.Add(node);
            for (var index = node.Children.Length - 1; index >= 0; index--) pending.Push(node.Children[index]);
        }
        foreach (var item in items)
            if (!item.Deleted && item.FolderId is { } id && byId.TryGetValue(id, out var node)) node.DirectItemCount++;
        foreach (var node in all) node.ItemCount = node.DirectItemCount;
        for (var index = all.Count - 1; index >= 0; index--)
            if (all[index].Parent is { } parent) parent.ItemCount += all[index].ItemCount;
        return new(roots.ToArray(), all.ToArray(), byId);
    }
}

// Public getters are also used by compiled WinUI bindings; these rows contain only list metadata.
public sealed record FolderRow(string Id, string Name, string DisplayName, int ItemCount, int DirectItemCount,
    int NestedFolders, bool CanEdit)
{
    public string ItemCountLabel => NestedFolders > 0
        ? $"{ItemCount} 个条目（含子文件夹），{NestedFolders} 个子文件夹" : $"{ItemCount} 个条目";
    public string DisplayText => FolderDisplay.Compact(DisplayName);
    public string OpenAutomationId => $"FolderOpen_{Id}";
    public string OpenLabel => $"打开文件夹“{Name}”";
    public string RenameLabel => $"重命名文件夹“{Name}”";
    public string DeleteLabel => $"删除文件夹“{Name}”";
    public override string ToString() => string.IsNullOrWhiteSpace(Name) ? "—" : Name;
}

// Data binding avoids eagerly constructing one ComboBoxItem per folder in application code.
// WinUI owns container realization; UIA traversal can still request additional containers.
public sealed record FolderChoice(string Id, string Name)
{
    public string DisplayName => string.IsNullOrWhiteSpace(Name) ? "—" : Name;
    public string DisplayText => FolderDisplay.Compact(DisplayName);
    public override string ToString() => DisplayName;
}

public sealed record FolderCrumb(string? Id, string Name)
{
    public string DisplayText => FolderDisplay.Compact(Name);
    public override string ToString() => Name;
}

internal static class FolderDisplay
{
    // Display-only abbreviation bounds text-layout/formatting work. Names/IDs and UIA names stay whole.
    internal static string Compact(string name)
    {
        if (name.Length <= 160) return name;
        var head = char.IsHighSurrogate(name[63]) && char.IsLowSurrogate(name[64]) ? 63 : 64;
        var tail = name.Length - 94;
        if (char.IsLowSurrogate(name[tail]) && char.IsHighSurrogate(name[tail - 1])) tail--;
        return string.Concat(name.AsSpan(0, head), "…".AsSpan(), name.AsSpan(tail));
    }
}
