using System.Runtime.InteropServices;
using System.Text.Json;

namespace Bitwarden_WinUI.Interop;

internal sealed record VaultAccount(string Id, string Email, uint ServerMode, string ServerUrl,
    VaultSessionState State, bool Active, bool NeedsSignIn = false, bool HelloEnabled = false)
{
    internal string ServerLabel => ServerMode switch
    {
        0 => "Bitwarden.com",
        1 => "Bitwarden.eu",
        _ => ServerUrl,
    };

    internal string CompactServerLabel => ServerMode switch
    {
        0 => "Bitwarden.com",
        1 => "Bitwarden.eu",
        _ => FormatCompactServer(ServerUrl),
    };

    internal static string FormatCompactServer(string serverUrl)
    {
        if (string.IsNullOrWhiteSpace(serverUrl)) return string.Empty;
        var text = serverUrl.Trim();
        if (Uri.TryCreate(text, UriKind.Absolute, out var uri) && !string.IsNullOrEmpty(uri.Host))
        {
            var host = uri.IsDefaultPort ? uri.Host : $"{uri.Host}:{uri.Port}";
            var path = uri.AbsolutePath.TrimEnd('/');
            text = string.IsNullOrEmpty(path) ? host : path.Length <= 12 ? host + path : host + "/…";
        }
        return text.Length <= 36 ? text : string.Concat(text.AsSpan(0, 35), "…");
    }

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
        using var buffer = new Utf8Buffer(JsonSerializer.Serialize(targets, JsonInfo<VaultAccountTimeout[]>()));
        return ReadJson<VaultAccount[]>(ApplyTimeoutsNative(buffer.Pointer, interrupt ? (byte)1 : (byte)0));
    }

    internal static void CancelPendingLogin() => CancelLoginNative();

    internal static void LogoutAll()
    {
        if (LogoutAllNative() == 0) throw new VaultException("cacheRevocationFailed");
    }
}
