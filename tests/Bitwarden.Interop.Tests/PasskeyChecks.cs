using Bitwarden_WinUI.Interop;
using System.Text.Json;

internal static class PasskeyChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var json = """
            {"id":"signed-request","creating":false,"relyingParty":"example.invalid",
             "sessionState":1,"choices":[],"needsSync":true}
            """;
        var pending = JsonSerializer.Deserialize<PasskeyPending>(json, new JsonSerializerOptions(JsonSerializerDefaults.Web))!;
        check(pending.IsFor("signed-request") && pending.NeedsSync && !pending.IsReadyFor("signed-request"),
            "unsynced passkey polling preserves its signed request identity and requests initial sync");
        check(!pending.IsFor("newer-request") && !pending.IsFor(""),
            "finishing passkey sign-in cannot adopt a replacement browser request");
        check((pending with { NeedsSync = false }).IsReadyFor("signed-request"),
            "a synchronized unlocked vault can advance the same passkey request");
        check(!(pending with { NeedsSync = false, SessionState = VaultSessionState.Locked }).IsReadyFor("signed-request"),
            "passkey synchronization cannot advance a subsequently locked session");
    }
}
