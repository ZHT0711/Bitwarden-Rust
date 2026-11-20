using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

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

    [DllImport(LibraryName, EntryPoint = "bw_auth_login", CallingConvention = CallingConvention.Cdecl)]
    private static extern uint LoginNative(
        IntPtr email,
        IntPtr password,
        uint provider,
        IntPtr token,
        uint serverMode,
        IntPtr serverUrl);

    [DllImport(LibraryName, EntryPoint = "bw_auth_send_two_factor_email", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte SendTwoFactorEmailNative(
        IntPtr email,
        IntPtr password,
        uint serverMode,
        IntPtr serverUrl);

    [DllImport(LibraryName, EntryPoint = "bw_auth_is_authenticated", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte IsAuthenticatedNative();

    [DllImport(LibraryName, EntryPoint = "bw_auth_logout", CallingConvention = CallingConvention.Cdecl)]
    private static extern void LogoutNative();

    internal static string Version => Marshal.PtrToStringUTF8(GetVersionNative()) ?? "unknown";
    internal static bool IsAuthenticated => IsAuthenticatedNative() != 0;

    internal static AuthResult Login(
        string email,
        string password,
        uint provider,
        string token,
        uint serverMode,
        string serverUrl)
    {
        var emailPointer = AllocateUtf8(email, out var emailLength);
        var passwordPointer = AllocateUtf8(password, out var passwordLength);
        var tokenPointer = AllocateUtf8(token, out var tokenLength);
        var serverPointer = AllocateUtf8(serverUrl, out var serverLength);

        try
        {
            var result = LoginNative(emailPointer, passwordPointer, provider, tokenPointer, serverMode, serverPointer);
            return new AuthResult(result & 0xff, result >> 8);
        }
        finally
        {
            ZeroAndFree(emailPointer, emailLength);
            ZeroAndFree(passwordPointer, passwordLength);
            ZeroAndFree(tokenPointer, tokenLength);
            ZeroAndFree(serverPointer, serverLength);
        }
    }

    internal static void Logout() => LogoutNative();

    internal static bool SendTwoFactorEmail(string email, string password, uint serverMode, string serverUrl)
    {
        var emailPointer = AllocateUtf8(email, out var emailLength);
        var passwordPointer = AllocateUtf8(password, out var passwordLength);
        var serverPointer = AllocateUtf8(serverUrl, out var serverLength);
        try
        {
            return SendTwoFactorEmailNative(emailPointer, passwordPointer, serverMode, serverPointer) != 0;
        }
        finally
        {
            ZeroAndFree(emailPointer, emailLength);
            ZeroAndFree(passwordPointer, passwordLength);
            ZeroAndFree(serverPointer, serverLength);
        }
    }

    private static IntPtr AllocateUtf8(string value, out int byteLength)
    {
        var bytes = Encoding.UTF8.GetBytes(value);
        byteLength = bytes.Length;
        var pointer = IntPtr.Zero;
        try
        {
            pointer = Marshal.AllocHGlobal(byteLength + 1);
            Marshal.Copy(bytes, 0, pointer, byteLength);
            Marshal.WriteByte(pointer, byteLength, 0);
            return pointer;
        }
        catch
        {
            if (pointer != IntPtr.Zero)
            {
                Marshal.FreeHGlobal(pointer);
            }
            throw;
        }
        finally
        {
            CryptographicOperations.ZeroMemory(bytes);
        }
    }

    private static void ZeroAndFree(IntPtr pointer, int byteLength)
    {
        if (pointer == IntPtr.Zero)
        {
            return;
        }

        try
        {
            for (var index = 0; index <= byteLength; index++)
            {
                Marshal.WriteByte(pointer, index, 0);
            }
        }
        finally
        {
            Marshal.FreeHGlobal(pointer);
        }
    }

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

internal readonly record struct AuthResult(uint Status, uint AvailableTwoFactorMethods);
