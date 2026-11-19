using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static class RustCore
{
    private const string LibraryName = "bitwarden_core.dll";

    [DllImport(LibraryName, EntryPoint = "bw_core_version", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GetVersionNative();

    [DllImport(LibraryName, EntryPoint = "bw_generate_password", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GeneratePasswordNative(uint length, uint options, uint minimumNumbers, uint minimumSpecial);

    [DllImport(LibraryName, EntryPoint = "bw_generate_passphrase", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GeneratePassphraseNative(
        uint numberOfWords,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string wordSeparator,
        byte capitalize,
        byte includeNumber);

    [DllImport(LibraryName, EntryPoint = "bw_string_free", CallingConvention = CallingConvention.Cdecl)]
    private static extern void FreeStringNative(IntPtr value);

    internal static string Version => Marshal.PtrToStringUTF8(GetVersionNative()) ?? "unknown";

    internal static string GeneratePassword(
        uint length,
        bool includeUppercase,
        bool includeLowercase,
        bool includeNumbers,
        bool includeSpecial,
        bool avoidAmbiguous,
        uint minimumNumbers,
        uint minimumSpecial)
    {
        uint options = 0;
        if (includeUppercase) options |= 1;
        if (includeLowercase) options |= 2;
        if (includeNumbers) options |= 4;
        if (includeSpecial) options |= 8;
        if (avoidAmbiguous) options |= 16;

        var value = GeneratePasswordNative(length, options, minimumNumbers, minimumSpecial);
        if (value == IntPtr.Zero)
        {
            throw new InvalidOperationException("Password settings are invalid or require more characters than the selected length.");
        }

        try
        {
            return Marshal.PtrToStringUTF8(value) ?? throw new InvalidOperationException("Rust core returned invalid UTF-8.");
        }
        finally
        {
            FreeStringNative(value);
        }
    }

    internal static string GeneratePassphrase(
        uint numberOfWords,
        string wordSeparator,
        bool capitalize,
        bool includeNumber)
    {
        var value = GeneratePassphraseNative(
            numberOfWords,
            wordSeparator,
            capitalize ? (byte)1 : (byte)0,
            includeNumber ? (byte)1 : (byte)0);
        if (value == IntPtr.Zero)
        {
            throw new InvalidOperationException("Passphrase settings are invalid. Choose between 3 and 20 words.");
        }

        try
        {
            return Marshal.PtrToStringUTF8(value) ?? throw new InvalidOperationException("Rust core returned invalid UTF-8.");
        }
        finally
        {
            FreeStringNative(value);
        }
    }
}
