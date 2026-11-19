using System.Runtime.InteropServices;

namespace Bitwarden_WinUI.Interop;

internal static class RustCore
{
    private const string LibraryName = "bitwarden_core.dll";

    [DllImport(LibraryName, EntryPoint = "bw_core_version", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GetVersionNative();

    [DllImport(LibraryName, EntryPoint = "bw_generate_password", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GeneratePasswordNative(uint length, uint options, uint minimumNumbers, uint minimumSpecial);

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
}
