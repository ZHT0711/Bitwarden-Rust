using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.Json.Serialization.Metadata;

namespace Bitwarden_WinUI.Interop;

[JsonSourceGenerationOptions(JsonSerializerDefaults.Web)]
[JsonSerializable(typeof(GeneratorSettings))]
internal sealed partial class GeneratorJsonContext : JsonSerializerContext { }

// Keep the existing PascalCase timeout preference format for stored settings.
[JsonSerializable(typeof(VaultTimeoutSettings))]
internal sealed partial class TimeoutJsonContext : JsonSerializerContext { }

internal static partial class RustCore
{
    private sealed record VaultMoveRequest(string[] Ids, string? FolderId);
    private sealed record PasskeyResponseRequest(string Id, string ItemId, string Revision, int CredentialIndex);

    // Closed response types are deliberate: trimmed builds must never fall back to reflection.
    [JsonSourceGenerationOptions(JsonSerializerDefaults.Web)]
    [JsonSerializable(typeof(NativeResponse<bool>))]
    [JsonSerializable(typeof(NativeResponse<string>))]
    [JsonSerializable(typeof(NativeResponse<PasskeyPending>))]
    [JsonSerializable(typeof(NativeResponse<VaultAccount[]>))]
    [JsonSerializable(typeof(NativeResponse<VaultOverview>))]
    [JsonSerializable(typeof(NativeResponse<VaultDetail>))]
    [JsonSerializable(typeof(NativeResponse<TotpCode>))]
    [JsonSerializable(typeof(NativeResponse<VaultEditorData>))]
    [JsonSerializable(typeof(NativeResponse<VaultMutationResult>))]
    [JsonSerializable(typeof(NativeResponse<SendOverview>))]
    [JsonSerializable(typeof(NativeResponse<SendDetail>))]
    [JsonSerializable(typeof(NativeResponse<VaultExportReport>))]
    [JsonSerializable(typeof(VaultExportRequest))]
    [JsonSerializable(typeof(SendRequest))]
    [JsonSerializable(typeof(VaultEditorData))]
    [JsonSerializable(typeof(VaultAccountTimeout[]))]
    [JsonSerializable(typeof(string[]))]
    [JsonSerializable(typeof(VaultMoveRequest))]
    [JsonSerializable(typeof(PasskeyResponseRequest))]
    private sealed partial class NativeJsonContext : JsonSerializerContext { }

    private static JsonTypeInfo<T> JsonInfo<T>() => NativeJsonMetadata<T>.Value;

    private static class NativeJsonMetadata<T>
    {
        // Resolve once for each closed type; request handling only reads the cached metadata.
        internal static readonly JsonTypeInfo<T> Value =
            (JsonTypeInfo<T>)(NativeJsonContext.Default.GetTypeInfo(typeof(T))
                ?? throw new InvalidOperationException("Native JSON type is not registered."));
    }
}
