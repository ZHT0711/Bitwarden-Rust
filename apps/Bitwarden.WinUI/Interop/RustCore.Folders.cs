using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bitwarden_WinUI.Interop;

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_folder_create", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr FolderCreateNative(IntPtr name, ulong requestId);
    [DllImport(LibraryName, EntryPoint = "bw_folder_rename", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr FolderRenameNative(IntPtr id, IntPtr name, ulong requestId);
    [DllImport(LibraryName, EntryPoint = "bw_folders_delete", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr FoldersDeleteNative(IntPtr ids, ulong requestId);
    [DllImport(LibraryName, EntryPoint = "bw_vault_move_items", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultMoveItemsNative(IntPtr data, ulong requestId);

    internal static VaultMutationResult CreateFolder(string name, CancellationToken token)
    {
        using var nameBuffer = new Utf8Buffer(name);
        return RunVaultRequest<VaultMutationResult>(requestId => FolderCreateNative(nameBuffer.Pointer, requestId), token, acknowledgedWrite: true);
    }

    internal static VaultMutationResult RenameFolder(string id, string name, CancellationToken token)
    {
        using var idBuffer = new Utf8Buffer(id);
        using var nameBuffer = new Utf8Buffer(name);
        return RunVaultRequest<VaultMutationResult>(requestId => FolderRenameNative(idBuffer.Pointer, nameBuffer.Pointer, requestId), token, acknowledgedWrite: true);
    }

    internal static VaultMutationResult DeleteFolders(string[] ids, CancellationToken token)
    {
        using var idsBuffer = new Utf8Buffer(JsonSerializer.Serialize(ids, JsonInfo<string[]>()));
        return RunVaultRequest<VaultMutationResult>(requestId => FoldersDeleteNative(idsBuffer.Pointer, requestId), token, acknowledgedWrite: true);
    }

    internal static VaultMutationResult MoveVaultItems(string[] ids, string? folderId, CancellationToken token)
    {
        using var data = new Utf8Buffer(JsonSerializer.Serialize(new VaultMoveRequest(ids, folderId), JsonInfo<VaultMoveRequest>()));
        return RunVaultRequest<VaultMutationResult>(requestId => VaultMoveItemsNative(data.Pointer, requestId), token, acknowledgedWrite: true);
    }
}
