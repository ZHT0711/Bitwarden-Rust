using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

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
        IntPtr serverUrl,
        ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_auth_send_two_factor_email", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte SendTwoFactorEmailNative(
        IntPtr email,
        IntPtr password,
        uint serverMode,
        IntPtr serverUrl,
        ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_auth_begin_request", CallingConvention = CallingConvention.Cdecl)]
    private static extern ulong BeginAuthRequestNative();

    [DllImport(LibraryName, EntryPoint = "bw_auth_cancel_request", CallingConvention = CallingConvention.Cdecl)]
    private static extern void CancelAuthRequestNative(ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_auth_validate_server", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte ValidateServerNative(uint serverMode, [MarshalAs(UnmanagedType.LPUTF8Str)] string serverUrl);

    [DllImport(LibraryName, EntryPoint = "bw_auth_is_authenticated", CallingConvention = CallingConvention.Cdecl)]
    private static extern byte IsAuthenticatedNative();

    [DllImport(LibraryName, EntryPoint = "bw_auth_logout", CallingConvention = CallingConvention.Cdecl)]
    private static extern void LogoutNative();

    [DllImport(LibraryName, EntryPoint = "bw_vault_overview", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultOverviewNative();

    [DllImport(LibraryName, EntryPoint = "bw_vault_sync", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultSyncNative(ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_vault_detail", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultDetailNative(IntPtr id, IntPtr password, ulong requestId);

    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    internal static VaultOverview GetVaultOverview() => ReadJson<VaultOverview>(VaultOverviewNative());

    internal static VaultOverview SyncVault(CancellationToken token) =>
        RunVaultRequest<VaultOverview>(VaultSyncNative, token);

    internal static VaultDetail GetVaultDetail(string id, string password, CancellationToken token)
    {
        using var idBuffer = new Utf8Buffer(id);
        using var passwordBuffer = new Utf8Buffer(password);
        return RunVaultRequest<VaultDetail>(requestId => VaultDetailNative(idBuffer.Pointer, passwordBuffer.Pointer, requestId), token);
    }

    private static T RunVaultRequest<T>(Func<ulong, IntPtr> operation, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        var requestId = BeginAuthRequestNative();
        using var cancellation = token.Register(() => CancelAuthRequestNative(requestId));
        var result = ReadJson<T>(operation(requestId));
        token.ThrowIfCancellationRequested();
        return result;
    }

    private static T ReadJson<T>(IntPtr pointer)
    {
        if (pointer == IntPtr.Zero) throw new VaultException("internal");
        try
        {
            var json = Marshal.PtrToStringUTF8(pointer) ?? throw new VaultException("internal");
            var response = JsonSerializer.Deserialize<NativeResponse<T>>(json, JsonOptions);
            if (response is null) throw new VaultException("internal");
            if (!response.Ok)
            {
                if (response.Error == "cancelled") throw new OperationCanceledException();
                throw new VaultException(response.Error ?? "internal");
            }
            return response.Data ?? throw new VaultException("internal");
        }
        catch (JsonException) { throw new VaultException("internal"); }
        finally { FreeStringNative(pointer); }
    }

    private sealed record NativeResponse<T>(bool Ok, T? Data, string? Error);

    internal static string Version => Marshal.PtrToStringUTF8(GetVersionNative()) ?? "unknown";
    internal static bool IsAuthenticated => IsAuthenticatedNative() != 0;
    internal static bool IsValidServer(uint serverMode, string serverUrl) => ValidateServerNative(serverMode, serverUrl) != 0;

    internal static AuthResult Login(
        string email,
        string password,
        uint provider,
        string token,
        uint serverMode,
        string serverUrl,
        CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        using var emailBuffer = new Utf8Buffer(email);
        using var passwordBuffer = new Utf8Buffer(password);
        using var tokenBuffer = new Utf8Buffer(token);
        using var serverBuffer = new Utf8Buffer(serverUrl);
        var requestId = BeginAuthRequestNative();
        using var cancellation = cancellationToken.Register(() => CancelAuthRequestNative(requestId));

        var result = LoginNative(emailBuffer.Pointer, passwordBuffer.Pointer, provider, tokenBuffer.Pointer,
            serverMode, serverBuffer.Pointer, requestId);
        if (cancellationToken.IsCancellationRequested)
        {
            LogoutNative();
            return new AuthResult(AuthStatus.Cancelled, 0);
        }
        return new AuthResult((AuthStatus)(result & 0xff), result >> 8);
    }

    internal static void Logout() => LogoutNative();

    internal static AuthStatus SendTwoFactorEmail(
        string email,
        string password,
        uint serverMode,
        string serverUrl,
        CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        using var emailBuffer = new Utf8Buffer(email);
        using var passwordBuffer = new Utf8Buffer(password);
        using var serverBuffer = new Utf8Buffer(serverUrl);
        var requestId = BeginAuthRequestNative();
        using var cancellation = cancellationToken.Register(() => CancelAuthRequestNative(requestId));
        var status = (AuthStatus)SendTwoFactorEmailNative(emailBuffer.Pointer, passwordBuffer.Pointer,
            serverMode, serverBuffer.Pointer, requestId);
        if (cancellationToken.IsCancellationRequested)
        {
            LogoutNative();
            return AuthStatus.Cancelled;
        }
        return status;
    }

    private sealed class Utf8Buffer : IDisposable
    {
        private readonly int _byteLength;
        public IntPtr Pointer { get; private set; }

        public Utf8Buffer(string value) => Pointer = AllocateUtf8(value, out _byteLength);

        public void Dispose()
        {
            ZeroAndFree(Pointer, _byteLength);
            Pointer = IntPtr.Zero;
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

internal enum AuthStatus : uint
{
    Failed = 0,
    Authenticated = 1,
    TwoFactorRequired = 2,
    TimedOut = 3,
    Cancelled = 4,
}

internal readonly record struct AuthResult(AuthStatus Status, uint AvailableTwoFactorMethods);
