using Bitwarden_WinUI.Interop;
using System.Text.Json;

internal static class GeneratorSettingsChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var defaults = GeneratorSettings.FromJson(null);
        check(defaults.PasswordLength == 14 && defaults.Uppercase && defaults.Lowercase && defaults.Numbers && !defaults.Special &&
            defaults.MinimumNumbers == 1 && defaults.MinimumSpecial == 0 && !defaults.AvoidAmbiguous,
            "generator restores official password defaults");
        check(defaults.PassphraseWords == 6 && defaults.WordSeparator == "-" && !defaults.CapitalizeWords && !defaults.IncludeNumber,
            "generator restores official passphrase defaults");

        var chosen = defaults with { SelectedType = 1, PasswordLength = 37, Special = true, MinimumNumbers = 3,
            MinimumSpecial = 2, AvoidAmbiguous = true, PassphraseWords = 9, WordSeparator = "_", CapitalizeWords = true, IncludeNumber = true };
        check(GeneratorSettings.FromJson(chosen.ToJson()) == chosen, "all non-secret generator options round-trip together");
        foreach (var invalid in new[] { "not json", "null", "[]", """{"passwordLength":"invalid"}""", """{"version":2}""", new string('x', 4097) })
            check(GeneratorSettings.FromJson(invalid) == defaults, "invalid or unsupported stored generator options restore defaults");

        var repaired = GeneratorSettings.FromJson("""{"selectedType":19,"passwordLength":-5,"minimumNumbers":99,"minimumSpecial":99,"special":true,"passphraseWords":200}""");
        check(repaired.SelectedType == 0 && repaired.MinimumNumbers == 9 && repaired.MinimumSpecial == 9 && repaired.PasswordLength == 20 && repaired.PassphraseWords == 20,
            "restored generator options respect limits and fit required character counts");
        var low = (defaults with { PasswordLength = 999, PassphraseWords = -1, MinimumNumbers = -1, MinimumSpecial = 4, Numbers = false, Special = false }).Normalize();
        check(low.PasswordLength == 128 && low.PassphraseWords == 3 && low.MinimumNumbers == 0 && low.MinimumSpecial == 0,
            "disabled character sets restore with zero minimums and supported ranges");
        check((defaults with { Uppercase = false, Lowercase = false, Numbers = false, Special = false }).Normalize().Lowercase,
            "restoration repairs an empty character pool before generation");
        check(GeneratorSettings.FromJson("""{"wordSeparator":null}""").WordSeparator == "-" &&
            GeneratorSettings.FromJson("""{"wordSeparator":""}""").WordSeparator == "" &&
            GeneratorSettings.FromJson("""{"wordSeparator":"bad\u0000separator"}""").WordSeparator == "-",
            "separator restoration preserves empty input and repairs invalid control characters");
        var unicodeSeparator = (defaults with { WordSeparator = new string('a', 15) + "😀" }).Normalize().WordSeparator;
        check(unicodeSeparator == new string('a', 15), "separator truncation keeps a complete UTF-16 character boundary");

        var values = new Dictionary<string, object>();
        chosen.Save(values, "account-a");
        (chosen with { PasswordLength = 45 }).Save(values, "account-b");
        (chosen with { PasswordLength = 55 }).Save(values, null);
        check(GeneratorSettings.Load(values, "account-a").PasswordLength == 37 &&
            GeneratorSettings.Load(values, "account-b").PasswordLength == 45 &&
            GeneratorSettings.Load(values, null).PasswordLength == 55,
            "account generator options and anonymous options use separate storage keys");
        GeneratorSettings.ResetSelectedType(values, "account-a");
        check(GeneratorSettings.Load(values, "account-a") == (chosen with { SelectedType = 0 }) &&
            GeneratorSettings.Load(values, "account-b").SelectedType == 1 && GeneratorSettings.Load(values, null).SelectedType == 1,
            "logout resets only that account's selected algorithm and preserves generation parameters");
        var count = values.Count;
        GeneratorSettings.ResetSelectedType(values, "unknown-account");
        check(values.Count == count, "logout does not create unused generator settings records");

        var properties = JsonDocument.Parse(chosen.ToJson()).RootElement.EnumerateObject().Select(p => p.Name).ToHashSet();
        check(properties.SetEquals(new[] { "version", "selectedType", "passwordLength", "uppercase", "lowercase", "numbers", "special",
            "avoidAmbiguous", "minimumNumbers", "minimumSpecial", "passphraseWords", "wordSeparator", "capitalizeWords", "includeNumber" }),
            "persisted generator payload is limited to non-secret settings");
    }
}
