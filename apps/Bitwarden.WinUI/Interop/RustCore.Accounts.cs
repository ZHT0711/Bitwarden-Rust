using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bitwarden_WinUI.Interop;

internal sealed record VaultAccount(string Id, string Email, uint ServerMode, string ServerUrl,
    VaultSessionState State, bool Active, bool NeedsSignIn = false)
{
    internal string ServerLabel => ServerMode switch
    {
        0 => "Bitwarden.com",
        1 => "Bitwarden.eu",
        _ => ServerUrl,
    };

    internal string Scope => AccountSettingsScope.Create(ServerMode, ServerUrl, Email);
}

internal sealed record VaultAccountTimeout(string Id, bool LogOut);

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_auth_accounts", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr AccountsNative();
    [DllImport(LibraryName, EntryPoint = "bw_auth_switch_account", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr SwitchAccountNative(IntPtr id);
    [DllImport(LibraryName, EntryPoint = "bw_auth_cancel_login", CallingConvention = CallingConvention.Cdecl)]
    private static extern void CancelLoginNative();
    [DllImport(LibraryName, EntryPoint = "bw_auth_logout_all", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte LogoutAllNative();
    [DllImport(LibraryName, EntryPoint = "bw_auth_shutdown", CallingConvention = CallingConvention.Cdecl)]
    internal static extern void Shutdown();
    [DllImport(LibraryName, EntryPoint = "bw_auth_apply_timeouts", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr ApplyTimeoutsNative(IntPtr actions, byte interrupt);

    internal static VaultAccount[] GetAccounts() => ReadJson<VaultAccount[]>(AccountsNative());

    internal static VaultAccount[] SwitchAccount(string id)
    {
        using var buffer = new Utf8Buffer(id);
        return ReadJson<VaultAccount[]>(SwitchAccountNative(buffer.Pointer));
    }

    internal static VaultAccount[] ApplyAccountTimeouts(VaultAccountTimeout[] targets, bool interrupt)
    {
        using var buffer = new Utf8Buffer(JsonSerializer.Serialize(targets, JsonOptions));
        return ReadJson<VaultAccount[]>(ApplyTimeoutsNative(buffer.Pointer, interrupt ? (byte)1 : (byte)0));
    }

    internal static void CancelPendingLogin() => CancelLoginNative();

    internal static void LogoutAll()
    {
        if (LogoutAllNative() == 0) throw new VaultException("cacheRevocationFailed");
    }
}
