using Bytewarden.WinUI.Interop;
using System.Text.Json;
using System.Text.Json.Serialization;

[JsonSourceGenerationOptions(JsonSerializerDefaults.Web)]
[JsonSerializable(typeof(JsonElement))]
[JsonSerializable(typeof(VaultAttachment))]
[JsonSerializable(typeof(VaultDetail))]
[JsonSerializable(typeof(SendDetail))]
internal sealed partial class FixtureJsonContext : JsonSerializerContext;
