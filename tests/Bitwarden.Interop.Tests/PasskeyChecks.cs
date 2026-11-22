using Bitwarden_WinUI.Interop;

internal static class PasskeyChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var json = """
            {"id":"signed-request","creating":false,"relyingParty":"example.invalid",
             "sessionState":1,"choices":[],"needsSync":true}
            """;
        var pending = RustCore.ParsePasskeyForChecks(json);
        check(pending.IsFor("signed-request") && pending.NeedsSync && !pending.IsReadyFor("signed-request"),
            "unsynced passkey polling preserves its signed request identity and requests initial sync");
        check(!pending.IsFor("newer-request") && !pending.IsFor(""),
            "finishing passkey sign-in cannot adopt a replacement browser request");
        check((pending with { NeedsSync = false }).IsReadyFor("signed-request"),
            "a synchronized unlocked vault can advance the same passkey request");
        check(!(pending with { NeedsSync = false, SessionState = VaultSessionState.Locked }).IsReadyFor("signed-request"),
            "passkey synchronization cannot advance a subsequently locked session");
        var blocked = RustCore.ParsePasskeyForChecks("""
            {"id":"protected-request","creating":true,"sessionState":1,"blockedReason":"ownVault"}
            """);
        check(blocked.BlockedReason == "ownVault" && blocked.IsFor("protected-request") && !blocked.IsReadyFor("protected-request"),
            "own-vault passkey refusal preserves request identity without allowing approval or requiring synchronization");
    }
}
