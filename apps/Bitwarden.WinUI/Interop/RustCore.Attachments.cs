using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_vault_download_attachment", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr DownloadAttachmentNative(IntPtr attachmentId, IntPtr destination, ulong detailRequestId);

    internal static void DownloadVaultAttachment(string attachmentId, string destination, ulong detailRequestId,
        CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        using var attachment = new Utf8Buffer(attachmentId);
        using var path = new Utf8Buffer(destination);
        using var registration = token.Register(() => CancelAuthRequestNative(detailRequestId));
        ReadJson<bool>(DownloadAttachmentNative(attachment.Pointer, path.Pointer, detailRequestId));
    }
}
