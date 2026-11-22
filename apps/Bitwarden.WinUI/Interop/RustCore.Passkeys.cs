using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bitwarden_WinUI.Interop;

internal sealed record PasskeyChoice(string Id, string Revision, int CredentialIndex, string Name, string? UserName, bool Reprompt)
{
    public override string ToString() => string.IsNullOrWhiteSpace(UserName) ? Name : $"{Name} · {UserName}";
}
internal sealed record PasskeyPending(string Id, bool Creating, string? RelyingParty, string? UserName,
    VaultSessionState SessionState, List<PasskeyChoice>? Choices, bool NeedsSync = false, string? BlockedReason = null)
{
    internal bool IsFor(string requestId) => requestId.Length != 0 && Id == requestId;
    internal bool IsReadyFor(string requestId) => IsFor(requestId) && SessionState == VaultSessionState.Unlocked && !NeedsSync && BlockedReason is null;
}

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_passkey_start", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyStartNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_register", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyRegisterNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_state", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyStateNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_sync_metadata", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeySyncMetadataNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_pending", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyPendingNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_respond", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyRespondNative(IntPtr json, IntPtr password, nuint window);
    [DllImport(LibraryName, EntryPoint = "bw_passkey_cancel", CallingConvention = CallingConvention.Cdecl)]
    private static extern void PasskeyCancelNative(IntPtr id);
    [DllImport(LibraryName, EntryPoint = "bw_passkey_dismiss_refusal", CallingConvention = CallingConvention.Cdecl)]
    private static extern void PasskeyDismissRefusalNative(IntPtr id);
    [DllImport(LibraryName, EntryPoint = "bw_passkey_active", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte PasskeyActiveNative(IntPtr id);

    internal static void StartPasskeys() => ReadJson<bool>(PasskeyStartNative());
    internal static void RegisterPasskeys() => ReadJson<bool>(PasskeyRegisterNative());
    internal static string PasskeyState() => ReadJson<string>(PasskeyStateNative());
    internal static bool SyncPasskeyMetadata() => ReadJson<bool>(PasskeySyncMetadataNative());
    internal static PasskeyPending PendingPasskey() => ReadJson<PasskeyPending>(PasskeyPendingNative());
    internal static bool IsPasskeyActive(string id)
    {
        using var buffer = new Utf8Buffer(id);
        return PasskeyActiveNative(buffer.Pointer) != 0;
    }
    internal static void CancelPasskey(string id)
    {
        using var buffer = new Utf8Buffer(id);
        PasskeyCancelNative(buffer.Pointer);
    }
    internal static void DismissPasskeyRefusal(string id)
    {
        using var buffer = new Utf8Buffer(id);
        PasskeyDismissRefusalNative(buffer.Pointer);
    }
    internal static void RespondPasskey(string requestId, PasskeyChoice? choice, string password, nint window)
    {
        var response = new PasskeyResponseRequest(requestId, choice?.Id ?? "", choice?.Revision ?? "", choice?.CredentialIndex ?? 0);
        using var json = new Utf8Buffer(JsonSerializer.Serialize(response, JsonInfo<PasskeyResponseRequest>()));
        using var secret = new Utf8Buffer(password);
        ReadJson<bool>(PasskeyRespondNative(json.Pointer, secret.Pointer, (nuint)window));
    }
}
