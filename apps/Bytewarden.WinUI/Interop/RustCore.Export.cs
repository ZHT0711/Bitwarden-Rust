using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bytewarden.WinUI.Interop;

internal sealed record VaultExportReport(int Items, int Folders, int SkippedRestricted, int OmittedAttachments, int OmittedUnsupported = 0);

internal static partial class RustCore
{
    private sealed record VaultExportRequest(string AccountId, string Destination, string Format, bool AllowPlaintext, string ExportPassword)
    {
        public override string ToString() => Format;
    }

    [DllImport(LibraryName, EntryPoint = "bw_vault_import", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultImportNative(IntPtr account, IntPtr source, IntPtr password, ulong requestId);

    internal static VaultMutationResult ImportVault(string account, string path, string password, CancellationToken token)
    {
        using var accountBuffer = new Utf8Buffer(account);
        using var source = new Utf8Buffer(path);
        using var protection = new Utf8Buffer(password);
        return RunVaultRequest<VaultMutationResult>(id => VaultImportNative(accountBuffer.Pointer, source.Pointer, protection.Pointer, id),
            token, acknowledgedWrite: true);
    }

    [DllImport(LibraryName, EntryPoint = "bw_vault_export", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultExportNative(IntPtr request, IntPtr password, ulong requestId);

    internal static VaultExportReport ExportVault(string account, string path, string format, bool allowPlaintext,
        string masterPassword, string exportPassword, CancellationToken token)
    {
        using var request = new Utf8Buffer(JsonSerializer.Serialize(new VaultExportRequest(account, path, format, allowPlaintext, exportPassword),
            JsonInfo<VaultExportRequest>()));
        using var password = new Utf8Buffer(masterPassword);
        return RunVaultRequest<VaultExportReport>(id => VaultExportNative(request.Pointer, password.Pointer, id), token, acknowledgedWrite: true);
    }
}
