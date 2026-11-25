using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Bytewarden.WinUI.Interop;

internal static partial class RustCore
{
    [DllImport(LibraryName, EntryPoint = "bw_vault_bulk_action", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultBulkActionNative(IntPtr json, IntPtr password, ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_password_strength", CallingConvention = CallingConvention.Cdecl)]
    private static extern int PasswordStrengthNative(IntPtr password, IntPtr username);

    [DllImport(LibraryName, EntryPoint = "bw_check_password_exposed", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr CheckPasswordExposedNative(IntPtr password, ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_generate_username", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr GenerateUsernameNative(IntPtr json, ulong requestId);

    [DllImport(LibraryName, EntryPoint = "bw_vault_fingerprint", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr VaultFingerprintNative();

    internal static VaultMutationResult BulkVaultAction(string[] ids, string action, string password, CancellationToken token)
    {
        using var jsonBuffer = new Utf8Buffer(JsonSerializer.Serialize(new VaultBulkActionRequest(ids, action), JsonInfo<VaultBulkActionRequest>()));
        using var passwordBuffer = new Utf8Buffer(password);
        return RunVaultRequest<VaultMutationResult>(
            id => VaultBulkActionNative(jsonBuffer.Pointer, passwordBuffer.Pointer, id),
            token,
            acknowledgedWrite: true);
    }

    internal static int CheckPasswordStrength(string password, string? username = null)
    {
        if (string.IsNullOrEmpty(password)) return -1;
        using var passwordBuffer = new Utf8Buffer(password);
        using var usernameBuffer = new Utf8Buffer(username ?? string.Empty);
        return PasswordStrengthNative(passwordBuffer.Pointer, usernameBuffer.Pointer);
    }

    internal static uint CheckPasswordExposed(string password, CancellationToken token)
    {
        using var passwordBuffer = new Utf8Buffer(password);
        return RunVaultRequest<uint>(id => CheckPasswordExposedNative(passwordBuffer.Pointer, id), token);
    }

    // SDK enum variant names are camelCase; fields inside variants are snake_case.
    internal static string GenerateUsernameWord(bool capitalize, bool includeNumber, CancellationToken token)
    {
        using var buffer = WriteUsernameJsonBuffer(writer =>
        {
            writer.WriteStartObject();
            writer.WriteStartObject("word");
            writer.WriteBoolean("capitalize", capitalize);
            writer.WriteBoolean("include_number", includeNumber);
            writer.WriteEndObject();
            writer.WriteEndObject();
        });
        return RunVaultRequest<string>(id => GenerateUsernameNative(buffer.Pointer, id), token);
    }

    internal static string GenerateUsernameSubaddress(string email, string? website, CancellationToken token)
    {
        using var buffer = WriteUsernameJsonBuffer(writer =>
        {
            writer.WriteStartObject();
            writer.WriteStartObject("subaddress");
            writer.WritePropertyName("type");
            WriteAppendType(writer, website);
            writer.WriteString("email", email);
            writer.WriteEndObject();
            writer.WriteEndObject();
        });
        return RunVaultRequest<string>(id => GenerateUsernameNative(buffer.Pointer, id), token);
    }

    internal static string GenerateUsernameCatchall(string domain, string? website, CancellationToken token)
    {
        using var buffer = WriteUsernameJsonBuffer(writer =>
        {
            writer.WriteStartObject();
            writer.WriteStartObject("catchall");
            writer.WritePropertyName("type");
            WriteAppendType(writer, website);
            writer.WriteString("domain", domain);
            writer.WriteEndObject();
            writer.WriteEndObject();
        });
        return RunVaultRequest<string>(id => GenerateUsernameNative(buffer.Pointer, id), token);
    }

    internal static string GenerateUsernameForwarded(
        string service,
        string apiKey,
        string? domain,
        string? baseUrl,
        string? website,
        CancellationToken token)
    {
        using var buffer = WriteUsernameJsonBuffer(writer =>
        {
            writer.WriteStartObject();
            writer.WriteStartObject("forwarded");
            writer.WriteStartObject("service");
            switch (service)
            {
                case "addyIo":
                    writer.WriteStartObject("addyIo");
                    writer.WriteString("api_token", apiKey);
                    writer.WriteString("domain", domain ?? string.Empty);
                    writer.WriteString("base_url", string.IsNullOrWhiteSpace(baseUrl) ? "https://app.addy.io" : baseUrl.Trim());
                    writer.WriteEndObject();
                    break;
                case "duckDuckGo":
                    writer.WriteStartObject("duckDuckGo");
                    writer.WriteString("token", apiKey);
                    writer.WriteEndObject();
                    break;
                case "firefox":
                    writer.WriteStartObject("firefox");
                    writer.WriteString("api_token", apiKey);
                    writer.WriteEndObject();
                    break;
                case "fastmail":
                    writer.WriteStartObject("fastmail");
                    writer.WriteString("api_token", apiKey);
                    writer.WriteEndObject();
                    break;
                case "forwardEmail":
                    writer.WriteStartObject("forwardEmail");
                    writer.WriteString("api_token", apiKey);
                    writer.WriteString("domain", domain ?? string.Empty);
                    writer.WriteEndObject();
                    break;
                case "simpleLogin":
                    writer.WriteStartObject("simpleLogin");
                    writer.WriteString("api_key", apiKey);
                    writer.WriteString("base_url", string.IsNullOrWhiteSpace(baseUrl) ? "https://app.simplelogin.io" : baseUrl.Trim());
                    writer.WriteEndObject();
                    break;
                default:
                    throw new VaultException("validation");
            }
            writer.WriteEndObject();
            if (string.IsNullOrWhiteSpace(website)) writer.WriteNull("website");
            else writer.WriteString("website", website.Trim());
            writer.WriteEndObject();
            writer.WriteEndObject();
        });
        return RunVaultRequest<string>(id => GenerateUsernameNative(buffer.Pointer, id), token);
    }

    internal static string GetAccountFingerprint() => ReadJson<string>(VaultFingerprintNative());

    private static void WriteAppendType(Utf8JsonWriter writer, string? website)
    {
        if (string.IsNullOrWhiteSpace(website))
        {
            writer.WriteStringValue("random");
            return;
        }
        writer.WriteStartObject();
        writer.WriteStartObject("websiteName");
        writer.WriteString("website", website.Trim());
        writer.WriteEndObject();
        writer.WriteEndObject();
    }

    private static Utf8Buffer WriteUsernameJsonBuffer(Action<Utf8JsonWriter> write)
    {
        using var stream = new MemoryStream();
        try
        {
            using (var writer = new Utf8JsonWriter(stream))
            {
                write(writer);
            }
            var json = Encoding.UTF8.GetString(stream.GetBuffer(), 0, (int)stream.Length);
            return new Utf8Buffer(json);
        }
        finally
        {
            if (stream.TryGetBuffer(out var segment) && segment.Array is not null)
                Array.Clear(segment.Array, segment.Offset, segment.Count);
        }
    }
}
