namespace Bitwarden_WinUI.Interop;

// Editable projections only. Cipher keys, passkey material, attachments and history stay in Rust.
internal sealed record VaultEditorData(string? Id, string Name, int ItemType, string? FolderId,
    bool Favorite, bool Reprompt, string? RevisionDate, string? Notes, Dictionary<string, string> Values,
    VaultUri[] Uris, VaultCustomField[] CustomFields, bool CanEdit, bool CanDelete, bool CanRestore,
    bool IsDeleted, bool IsArchived, bool CanArchive = false, string? EditBlockedReason = null);
internal sealed record VaultUri(string? Uri, int? Match);
internal sealed record VaultCustomField(string? Name, string? Value, int Type, int? LinkedId);
internal sealed record VaultMutationResult(VaultOverview Overview, string? Id);
