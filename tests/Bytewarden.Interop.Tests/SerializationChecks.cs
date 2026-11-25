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
        using (var document = JsonDocument.Parse(JsonForChecks(new VaultBulkActionRequest(["item-1", "item-2"], "trash"))))
            check(document.RootElement.GetProperty("ids").GetArrayLength() == 2 &&
                document.RootElement.GetProperty("action").GetString() == "trash" &&
                document.RootElement.EnumerateObject().Count() == 2, "generated bulk action JSON retains its exact native request shape");
        using (var document = JsonDocument.Parse(JsonForChecks(new PasskeyResponseRequest("request", "item", "revision", 2))))
            check(document.RootElement.GetProperty("id").GetString() == "request" &&
                document.RootElement.GetProperty("itemId").GetString() == "item" &&
                document.RootElement.GetProperty("credentialIndex").GetInt32() == 2 &&
                document.RootElement.EnumerateObject().Count() == 4, "generated passkey JSON retains its exact native request shape");

        CheckResponseMetadata<bool>(check, "boolean");
        CheckResponseMetadata<uint>(check, "unsigned integer");
        CheckResponseMetadata<string>(check, "string");
        CheckResponseMetadata<PasskeyPending>(check, "pending passkey");
        CheckResponseMetadata<VaultAccount[]>(check, "accounts");
        CheckResponseMetadata<VaultOverview>(check, "overview");
        CheckResponseMetadata<VaultDetail>(check, "detail");
        CheckResponseMetadata<TotpCode>(check, "TOTP");
        CheckResponseMetadata<VaultEditorData>(check, "editor");
        CheckResponseMetadata<VaultMutationResult>(check, "mutation");
        CheckResponseMetadata<VaultExportReport>(check, "export report");
        check(ParseJsonForChecks<NativeResponse<VaultDetail>>(
            "{\"ok\":true,\"data\":{\"name\":\"Login\",\"itemType\":1,\"fields\":[{\"label\":\"网站\",\"value\":\"example.com\",\"sensitive\":false,\"launchUri\":\"https://example.com/\"}],\"revisionDate\":\"2026-11-20T00:00:00Z\",\"restricted\":false,\"passwordHidden\":false,\"attachments\":0,\"creationDate\":\"2026-11-19T00:00:00Z\",\"passwordRevisionDate\":\"2026-11-19T12:00:00Z\",\"passwordHistory\":[{\"password\":\"old\",\"lastUsedDate\":\"2026-11-18T00:00:00Z\"}],\"passwordStrength\":4}}")
            .Data is { PasswordStrength: 4, CreationDate: not null, PasswordRevisionDate: not null, Fields: [{ LaunchUri: "https://example.com/" }], PasswordHistory: [{ Password: "old" }] },
            "generated detail metadata reads dates, launch URI, password history and strength");
        check(ParseJsonForChecks<NativeResponse<VaultAccount[]>>(
            "{\"ok\":true,\"data\":[{\"id\":\"11111111-1111-4111-8111-111111111111\",\"email\":\"a@example.invalid\",\"serverMode\":0,\"serverUrl\":\"\",\"state\":2,\"active\":true,\"pinEnabled\":true,\"pinPersistent\":false}]}")
            .Data is [{ PinEnabled: true, PinPersistent: false }],
            "generated account metadata reads PIN enrollment state without reflection");
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
