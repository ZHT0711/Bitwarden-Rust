using System.Text.Json;
using System.Text.Json.Serialization;

namespace Bitwarden_WinUI.Interop;

// Generator parameters are plaintext preferences. Generated credentials stay in the page.
[JsonUnmappedMemberHandling(JsonUnmappedMemberHandling.Disallow)]
internal sealed record GeneratorSettings
{
    private const string KeyPrefix = "GeneratorOptionsV1.";

    // Ordinary setters allow generated JSON deserialization to preserve defaults for missing fields.
    public int Version { get; set; } = 1;
    public int SelectedType { get; set; }
    public int PasswordLength { get; set; } = 14;
    public bool Uppercase { get; set; } = true;
    public bool Lowercase { get; set; } = true;
    public bool Numbers { get; set; } = true;
    public bool Special { get; set; }
    public bool AvoidAmbiguous { get; set; }
    public int MinimumNumbers { get; set; } = 1;
    public int MinimumSpecial { get; set; }
    public int PassphraseWords { get; set; } = 6;
    public string WordSeparator { get; set; } = "-";
    public bool CapitalizeWords { get; set; }
    public bool IncludeNumber { get; set; }

    internal GeneratorSettings Normalize()
    {
        var lowercase = Lowercase || !(Uppercase || Numbers || Special);
        var minimumNumbers = Numbers ? Math.Clamp(MinimumNumbers, 1, 9) : 0;
        var minimumSpecial = Special ? Math.Clamp(MinimumSpecial, 1, 9) : 0;
        var requiredCharacters = (Uppercase ? 1 : 0) + (lowercase ? 1 : 0) + minimumNumbers + minimumSpecial;
        return this with
        {
            Version = 1,
            SelectedType = SelectedType == 1 ? 1 : 0,
            PasswordLength = Math.Max(Math.Clamp(PasswordLength, 5, 128), requiredCharacters),
            Lowercase = lowercase,
            MinimumNumbers = minimumNumbers,
            MinimumSpecial = minimumSpecial,
            PassphraseWords = Math.Clamp(PassphraseWords, 3, 20),
            WordSeparator = NormalizeSeparator(WordSeparator),
        };
    }

    private static string NormalizeSeparator(string? value)
    {
        if (value is null || value.Any(char.IsControl)) return "-";
        var length = Math.Min(value.Length, 16);
        if (length > 0 && char.IsHighSurrogate(value[length - 1])) length--;
        return value[..length];
    }

    internal static GeneratorSettings FromJson(string? json)
    {
        if (string.IsNullOrEmpty(json) || json.Length > 4096) return new();
        try
        {
            var settings = JsonSerializer.Deserialize(json, GeneratorJsonContext.Default.GeneratorSettings);
            return settings?.Version == 1 ? settings.Normalize() : new();
        }
        catch (JsonException) { return new(); }
    }

    internal string ToJson() => JsonSerializer.Serialize(Normalize(), GeneratorJsonContext.Default.GeneratorSettings);

    internal static GeneratorSettings Load(IDictionary<string, object> values, string? scope) =>
        values.TryGetValue(KeyPrefix + (scope ?? "anonymous"), out var stored) && stored is string json
            ? FromJson(json) : new();

    internal void Save(IDictionary<string, object> values, string? scope) =>
        values[KeyPrefix + (scope ?? "anonymous")] = ToJson();

    internal static void ResetSelectedType(IDictionary<string, object> values, string scope)
    {
        if (values.ContainsKey(KeyPrefix + scope))
            (Load(values, scope) with { SelectedType = 0 }).Save(values, scope);
    }
}
