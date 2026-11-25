using System.Text.Json;

namespace Bytewarden.WinUI.Interop;

// This partial exists only in the test assembly and uses the app's generated metadata.
internal static partial class RustCore
{
    internal static string JsonForChecks<T>(T value) => JsonSerializer.Serialize(value, JsonInfo<T>());
    internal static T ParseJsonForChecks<T>(string json) => JsonSerializer.Deserialize(json, JsonInfo<T>())!;
    internal static PasskeyPending ParsePasskeyForChecks(string json) =>
        ParseJsonForChecks<NativeResponse<PasskeyPending>>("{\"ok\":true,\"data\":" + json + "}").Data!;

    internal static void CheckJsonContracts(Action<bool, string> check)
    {
        check(!JsonSerializer.IsReflectionEnabledByDefault, "managed checks run with JSON reflection disabled");
        var settings = new VaultTimeoutSettings(VaultTimeoutMode.SystemIdle, 23, VaultTimeoutAction.LogOut);
        var json = JsonSerializer.Serialize(settings, TimeoutJsonContext.Default.VaultTimeoutSettings);
        using (var document = JsonDocument.Parse(json))
            check(document.RootElement.TryGetProperty("Mode", out _) &&
                !document.RootElement.TryGetProperty("mode", out _), "timeout JSON preserves the existing PascalCase preference format");
        check(JsonSerializer.Deserialize(json, TimeoutJsonContext.Default.VaultTimeoutSettings) == settings,
            "timeout preferences round-trip through generated metadata");

        using (var document = JsonDocument.Parse(JsonForChecks(new VaultMoveRequest(["synthetic-item"], "synthetic-folder"))))
            check(document.RootElement.GetProperty("ids")[0].GetString() == "synthetic-item" &&
                document.RootElement.GetProperty("folderId").GetString() == "synthetic-folder" &&
                document.RootElement.EnumerateObject().Count() == 2, "generated folder move JSON retains its exact native request shape");
        using (var document = JsonDocument.Parse(JsonForChecks(new PasskeyResponseRequest("request", "item", "revision", 2))))
            check(document.RootElement.GetProperty("id").GetString() == "request" &&
                document.RootElement.GetProperty("itemId").GetString() == "item" &&
                document.RootElement.GetProperty("credentialIndex").GetInt32() == 2 &&
                document.RootElement.EnumerateObject().Count() == 4, "generated passkey JSON retains its exact native request shape");

        CheckResponseMetadata<bool>(check, "boolean");
        CheckResponseMetadata<string>(check, "string");
        CheckResponseMetadata<PasskeyPending>(check, "pending passkey");
        CheckResponseMetadata<VaultAccount[]>(check, "accounts");
        CheckResponseMetadata<VaultOverview>(check, "overview");
        CheckResponseMetadata<VaultDetail>(check, "detail");
        CheckResponseMetadata<TotpCode>(check, "TOTP");
        CheckResponseMetadata<VaultEditorData>(check, "editor");
        CheckResponseMetadata<VaultMutationResult>(check, "mutation");
        CheckResponseMetadata<VaultExportReport>(check, "export report");
        check(ParseJsonForChecks<NativeResponse<VaultExportReport>>(
            "{\"ok\":true,\"data\":{\"items\":2,\"folders\":1,\"skippedRestricted\":3,\"omittedAttachments\":4,\"omittedUnsupported\":5}}")
            .Data is { Items: 2, Folders: 1, SkippedRestricted: 3, OmittedAttachments: 4, OmittedUnsupported: 5 },
            "generated export metadata retains CSV omissions in trimmed builds");
    }

    private static void CheckResponseMetadata<T>(Action<bool, string> check, string label)
    {
        var response = ParseJsonForChecks<NativeResponse<T>>("{\"ok\":false,\"error\":\"notFound\"}");
        check(response is { Ok: false, Error: "notFound" }, $"generated metadata reads the {label} native response without reflection");
    }
}
