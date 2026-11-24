using System.Globalization;

namespace Bitwarden_WinUI.Interop;

internal enum VaultSort { NameAscending, NameDescending, RevisionNewest, RevisionOldest }

internal static class VaultQuery
{
    // Only list metadata is searched. Filtering must never open a secret detail or bypass reprompt.
    internal static VaultItem[] Apply(IEnumerable<VaultItem> source, string category, string folder, string query)
    {
        var type = category switch { "logins" => 1, "notes" => 2, "cards" => 3, "identities" => 4,
            "ssh" => 5, "bankaccounts" => 6, "licenses" => 7, "passports" => 8, _ => 0 };
        var terms = SearchTerms(query);
        return source.Where(item =>
            (category == "trash" ? item.Deleted : !item.Deleted) &&
            (category == "trash" || (category == "archived" ? item.Archived : !item.Archived)) &&
            (category != "favorites" || item.Favorite) && (type == 0 || item.ItemType == type) &&
            (folder == "*" || (item.FolderId ?? "") == folder) &&
            terms.All(term => item.Name.Contains(term, StringComparison.OrdinalIgnoreCase) ||
                item.Subtitle.Contains(term, StringComparison.OrdinalIgnoreCase))).ToArray();
    }

    internal static VaultItem[] Sort(IEnumerable<VaultItem> source, VaultSort sort) => sort switch
    {
        VaultSort.NameDescending => source.OrderByDescending(item => item.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ThenBy(item => item.Id, StringComparer.Ordinal).ToArray(),
        VaultSort.RevisionNewest or VaultSort.RevisionOldest => SortRevisions(source, sort),
        _ => source.OrderBy(item => item.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ThenBy(item => item.Id, StringComparer.Ordinal).ToArray(),
    };

    internal static bool SameProjection(VaultItem[]? previous, VaultItem[] current) => previous is not null &&
        previous.Length == current.Length && previous.Zip(current).All(pair => ReferenceEquals(pair.First, pair.Second));

    internal static VaultItem[] RetainSelection(VaultItem[] selected, VaultItem[] visible)
    {
        if (selected.Length == 0) return [];
        if (selected.Length == 1) return visible.Any(item => ReferenceEquals(item, selected[0])) ? selected : [];
        var available = new HashSet<VaultItem>(visible, ReferenceEqualityComparer.Instance);
        return selected.Where(available.Contains).ToArray();
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
        var terms = new List<string>();
        var term = new System.Text.StringBuilder();
        var quoted = false;
        foreach (var character in query)
        {
            if (character == '"') quoted = !quoted;
            else if (!quoted && char.IsWhiteSpace(character))
            {
                if (term.Length > 0) { terms.Add(term.ToString()); term.Clear(); }
            }
            else term.Append(character);
        }
        if (term.Length > 0) terms.Add(term.ToString());
        return terms.Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
    }
}
