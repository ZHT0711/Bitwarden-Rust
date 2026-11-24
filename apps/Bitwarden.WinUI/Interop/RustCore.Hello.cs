using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_hello_enable_for_window", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr HelloEnableNative(IntPtr account, IntPtr password, ulong windowId, ulong requestId);
    [DllImport(LibraryName, EntryPoint = "bw_hello_disable", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr HelloDisableNative(IntPtr account);
    [DllImport(LibraryName, EntryPoint = "bw_hello_unlock_for_window", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr HelloUnlockNative(IntPtr account, ulong windowId, ulong requestId);

    internal static void EnableHello(string account, string password, CancellationToken token, ulong windowId = 0)
    {
        using var accountBuffer = new Utf8Buffer(account);
        using var passwordBuffer = new Utf8Buffer(password);
        RunVaultRequest<bool>(id => HelloEnableNative(accountBuffer.Pointer, passwordBuffer.Pointer, windowId, id), token, acknowledgedWrite: true);
    }

    internal static void DisableHello(string account)
    {
        using var buffer = new Utf8Buffer(account);
        ReadJson<bool>(HelloDisableNative(buffer.Pointer));
    }

    internal static void UnlockHello(string account, CancellationToken token, ulong windowId = 0)
    {
        using var buffer = new Utf8Buffer(account);
        RunVaultRequest<bool>(id => HelloUnlockNative(buffer.Pointer, windowId, id), token, acknowledgedWrite: true);
    }
}
