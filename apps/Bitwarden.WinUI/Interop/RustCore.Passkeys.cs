using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bitwarden_WinUI.Interop;

internal sealed record PasskeyChoice(string Id, string Revision, int CredentialIndex, string Name, string? UserName, bool Reprompt)
{
    public override string ToString() => string.IsNullOrWhiteSpace(UserName) ? Name : $"{Name} · {UserName}";
}
internal sealed record PasskeyPending(string Id, bool Creating, string? RelyingParty, string? UserName,
    VaultSessionState SessionState, List<PasskeyChoice>? Choices);

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_passkey_start", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyStartNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_register", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyRegisterNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_state", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyStateNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_pending", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyPendingNative();
    [DllImport(LibraryName, EntryPoint = "bw_passkey_respond", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PasskeyRespondNative(IntPtr json, IntPtr password, nuint window);
    [DllImport(LibraryName, EntryPoint = "bw_passkey_cancel", CallingConvention = CallingConvention.Cdecl)]
    private static extern void PasskeyCancelNative(IntPtr id);
    [DllImport(LibraryName, EntryPoint = "bw_passkey_active", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte PasskeyActiveNative(IntPtr id);

    internal static void StartPasskeys() => ReadJson<bool>(PasskeyStartNative());
    internal static void RegisterPasskeys() => ReadJson<bool>(PasskeyRegisterNative());
    internal static string PasskeyState() => ReadJson<string>(PasskeyStateNative());
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
    internal static void RespondPasskey(string requestId, PasskeyChoice? choice, string password, nint window)
    {
        using var json = new Utf8Buffer(JsonSerializer.Serialize(new {
            id = requestId, itemId = choice?.Id ?? "", revision = choice?.Revision ?? "", credentialIndex = choice?.CredentialIndex ?? 0,
        }, JsonOptions));
        using var secret = new Utf8Buffer(password);
        ReadJson<bool>(PasskeyRespondNative(json.Pointer, secret.Pointer, (nuint)window));
    }
}
