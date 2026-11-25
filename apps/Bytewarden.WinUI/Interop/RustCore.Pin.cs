using System.Runtime.InteropServices;

namespace Bytewarden.WinUI.Interop;

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_pin_enable", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PinEnableNative(
        IntPtr account,
        IntPtr password,
        IntPtr pin,
        byte requireMasterPasswordOnRestart,
        ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_pin_disable", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PinDisableNative(IntPtr account);

    [DllImport(LibraryName, EntryPoint = "bw_pin_unlock", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr PinUnlockNative(IntPtr account, IntPtr pin, ulong requestId);

    internal static void EnablePin(
        string account,
        string masterPassword,
        string pin,
        bool requireMasterPasswordOnRestart,
        CancellationToken token)
    {
        using var accountBuffer = new Utf8Buffer(account);
        using var passwordBuffer = new Utf8Buffer(masterPassword);
        using var pinBuffer = new Utf8Buffer(pin);
        RunVaultRequest<bool>(
            id => PinEnableNative(
                accountBuffer.Pointer,
                passwordBuffer.Pointer,
                pinBuffer.Pointer,
                requireMasterPasswordOnRestart ? (byte)1 : (byte)0,
                id),
            token,
            acknowledgedWrite: true);
    }

    internal static void DisablePin(string account)
    {
        using var buffer = new Utf8Buffer(account);
        ReadJson<bool>(PinDisableNative(buffer.Pointer));
    }

    internal static void UnlockPin(string account, string pin, CancellationToken token)
    {
        using var accountBuffer = new Utf8Buffer(account);
        using var pinBuffer = new Utf8Buffer(pin);
        RunVaultRequest<bool>(
            id => PinUnlockNative(accountBuffer.Pointer, pinBuffer.Pointer, id),
            token,
            acknowledgedWrite: true);
    }
}
