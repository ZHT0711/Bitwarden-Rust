namespace Bitwarden_WinUI.Interop;

internal static class VaultQuery
{
    internal static VaultItem[] Apply(IEnumerable<VaultItem> source, string category, string folder, string query)
    {
        var type = category switch { "logins" => 1, "notes" => 2, "cards" => 3, "identities" => 4,
            "ssh" => 5, "bankaccounts" => 6, "licenses" => 7, "passports" => 8, _ => 0 };
        query = query.Trim();
        return source.Where(item =>
            (category == "trash" ? item.Deleted : !item.Deleted) &&
            (category == "trash" || (category == "archived" ? item.Archived : !item.Archived)) &&
            (category != "favorites" || item.Favorite) && (type == 0 || item.ItemType == type) &&
            (folder == "*" || (item.FolderId ?? "") == folder) &&
            (query.Length == 0 || item.Name.Contains(query, StringComparison.OrdinalIgnoreCase) || item.Subtitle.Contains(query, StringComparison.OrdinalIgnoreCase))).ToArray();
    }
}
