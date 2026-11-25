using System.Buffers;
using System.Globalization;

namespace Bytewarden.WinUI.Interop;

internal enum VaultSort { NameAscending, NameDescending, RevisionNewest, RevisionOldest }

internal static class VaultQuery
{
    // Only list metadata is searched. Filtering must never open a secret detail or bypass reprompt.
    internal static VaultItem[] Apply(IEnumerable<VaultItem> source, string category, string folder, string query)
    {
        var type = category switch { "logins" => 1, "notes" => 2, "cards" => 3, "identities" => 4,
            "ssh" => 5, "bankaccounts" => 6, "licenses" => 7, "passports" => 8, _ => 0 };
        var terms = SearchTerms(query);
        var isTrash = category == "trash";
        var isArchived = category == "archived";
        var isFavorites = category == "favorites";
        var allFolders = folder == "*";
        var hasTerms = terms.Length > 0;

        if (source is VaultItem[] array)
        {
            if (array.Length == 0) return [];
            var matchCount = 0;
            VaultItem[]? buffer = null;
            try
            {
                for (var index = 0; index < array.Length; index++)
                {
                    var item = array[index];
                    if ((isTrash ? item.Deleted : !item.Deleted) &&
                        (isTrash || (isArchived ? item.Archived : !item.Archived)) &&
                        (!isFavorites || item.Favorite) &&
                        (type == 0 || item.ItemType == type) &&
                        (allFolders || (item.FolderId ?? "") == folder) &&
                        (!hasTerms || MatchesMetadata(item, terms)))
                    {
                        if (buffer is not null) buffer[matchCount++] = item;
                        else matchCount++;
                    }
                    else if (buffer is null)
                    {
                        buffer = ArrayPool<VaultItem>.Shared.Rent(array.Length - index + matchCount);
                        if (matchCount > 0) Array.Copy(array, 0, buffer, 0, matchCount);
                    }
                }
                if (buffer is null) return array;
                if (matchCount == 0) return [];
                var result = new VaultItem[matchCount];
                Array.Copy(buffer, 0, result, 0, matchCount);
                return result;
            }
            finally
            {
                if (buffer is not null) ArrayPool<VaultItem>.Shared.Return(buffer, clearArray: true);
            }
        }

        var filtered = new List<VaultItem>();
        foreach (var item in source)
        {
            if ((isTrash ? item.Deleted : !item.Deleted) &&
                (isTrash || (isArchived ? item.Archived : !item.Archived)) &&
                (!isFavorites || item.Favorite) &&
                (type == 0 || item.ItemType == type) &&
                (allFolders || (item.FolderId ?? "") == folder) &&
                (!hasTerms || MatchesMetadata(item, terms)))
            {
                filtered.Add(item);
            }
        }
        return filtered.Count == 0 ? [] : filtered.ToArray();
    }

    private static bool MatchesMetadata(VaultItem item, string[] terms)
    {
        // A per-item terms.All(lambda) captures item and allocates a closure/delegate for every row.
        // Keep AND/ordinal metadata-only semantics without those O(items) transient allocations.
        for (var index = 0; index < terms.Length; index++)
        {
            var term = terms[index];
            if (!item.Name.Contains(term, StringComparison.OrdinalIgnoreCase) &&
                !item.Subtitle.Contains(term, StringComparison.OrdinalIgnoreCase)) return false;
        }
        return true;
    }

    internal static VaultItem[] Sort(IEnumerable<VaultItem> source, VaultSort sort) => sort switch
    {
        VaultSort.NameDescending => source.OrderByDescending(item => item.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ThenBy(item => item.Id, StringComparer.Ordinal).ToArray(),
        VaultSort.RevisionNewest or VaultSort.RevisionOldest => SortRevisions(source, sort),
        _ => source.OrderBy(item => item.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ThenBy(item => item.Id, StringComparer.Ordinal).ToArray(),
    };

    internal static bool SameProjection(VaultItem[]? previous, VaultItem[] current)
    {
        if (ReferenceEquals(previous, current)) return true;
        if (previous is null || previous.Length != current.Length) return false;
        for (var index = 0; index < current.Length; index++)
            if (!ReferenceEquals(previous[index], current[index])) return false;
        return true;
    }

    internal static VaultItem[] RetainSelection(VaultItem[] selected, VaultItem[] visible)
    {
        if (selected.Length == 0 || visible.Length == 0) return [];
        if (selected.Length == 1)
        {
            var target = selected[0];
            for (var index = 0; index < visible.Length; index++)
                if (ReferenceEquals(visible[index], target)) return selected;
            return [];
        }
        var available = new HashSet<VaultItem>(visible, ReferenceEqualityComparer.Instance);
        var kept = new List<VaultItem>(Math.Min(selected.Length, visible.Length));
        for (var index = 0; index < selected.Length; index++)
            if (available.Contains(selected[index])) kept.Add(selected[index]);
        return kept.Count == 0 ? [] : kept.ToArray();
    }

    private static VaultItem[] SortRevisions(IEnumerable<VaultItem> source, VaultSort sort)
    {
        // Parse each revision once, not on every comparison. Unknown dates always come last.
        var dated = source.Select(item => (Item: item, Revision: ParseRevision(item.RevisionDate)));
        var ordered = dated.OrderBy(entry => !entry.Revision.HasValue);
        return (sort == VaultSort.RevisionNewest
                ? ordered.ThenByDescending(entry => entry.Revision)
                : ordered.ThenBy(entry => entry.Revision))
            .ThenBy(entry => entry.Item.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ThenBy(entry => entry.Item.Id, StringComparer.Ordinal)
            .Select(entry => entry.Item).ToArray();
    }

    private static DateTimeOffset? ParseRevision(string revision) =>
        DateTimeOffset.TryParse(revision, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal, out var value) ? value : null;

    private static string[] SearchTerms(string query)
    {
        if (string.IsNullOrWhiteSpace(query)) return [];
        var hasSpecial = false;
        for (var index = 0; index < query.Length; index++)
        {
            var character = query[index];
            if (character == '"' || char.IsWhiteSpace(character)) { hasSpecial = true; break; }
        }
        if (!hasSpecial) return [query];

        var terms = new List<string>();
        var term = new System.Text.StringBuilder();
        var quoted = false;
        foreach (var character in query)
        {
            if (character == '"') quoted = !quoted;
            else if (!quoted && char.IsWhiteSpace(character))
            {
                if (term.Length > 0) { AddDistinct(terms, term.ToString()); term.Clear(); }
            }
            else term.Append(character);
        }
        if (term.Length > 0) AddDistinct(terms, term.ToString());
        return terms.Count == 0 ? [] : terms.ToArray();
    }

    private static void AddDistinct(List<string> terms, string candidate)
    {
        for (var index = 0; index < terms.Count; index++)
            if (string.Equals(terms[index], candidate, StringComparison.OrdinalIgnoreCase)) return;
        terms.Add(candidate);
    }
}
