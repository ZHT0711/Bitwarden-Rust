using Bytewarden.WinUI.Interop;

internal static class VaultTimeoutChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var timed = new VaultTimeoutSettings(VaultTimeoutMode.Inactivity, 1, VaultTimeoutAction.Lock);
        check(VaultTimeoutSettings.Default.Mode == VaultTimeoutMode.AppRestart, "desktop timeout defaults to restart like upstream");
        check(!timed.IsExpired(timed.Mode, VaultSessionState.Unlocked, false, 59999, 999999), "numeric timeout waits for its full duration");
        check(timed.IsExpired(timed.Mode, VaultSessionState.Unlocked, false, 60000, 0), "numeric timeout expires at the boundary even with other-app input");
        check(!timed.IsExpired(timed.Mode, VaultSessionState.Unlocked, true, 900000, 900000), "focused vault remains usable for numeric timeout like upstream");
        check(!timed.IsExpired(timed.Mode, VaultSessionState.Locked, false, 900000, 900000), "numeric timer does not reinterpret an already locked vault as a new timeout");
        check(!timed.IsExpired(timed.Mode, VaultSessionState.SignedOut, false, 900000, 900000), "signed-out session has no automatic timeout action");
        var idle = timed with { Mode = VaultTimeoutMode.SystemIdle };
        check(idle.IsExpired(idle.Mode, VaultSessionState.Unlocked, true, 0, 300000), "system-idle timeout applies while the vault is foreground");
        check(!idle.IsExpired(idle.Mode, VaultSessionState.Unlocked, false, 999999, null), "failed system-idle observation does not invent an idle period");
        check(!idle.CanApply(VaultSessionState.Locked) && (idle with { Action = VaultTimeoutAction.LogOut }).CanApply(VaultSessionState.Locked), "system events can log out a locked account without repeating lock");
        check((timed with { Minutes = int.MaxValue }).IsExpired(timed.Mode, VaultSessionState.Unlocked, false, (long)int.MaxValue * 60000, 0), "large custom timeout uses non-overflowing duration arithmetic");
        check((timed with { Mode = VaultTimeoutMode.SystemLock }).EffectiveMode(false, true) == VaultTimeoutMode.SystemSleep, "unavailable system-lock monitor promotes to sleep");
        check((timed with { Mode = VaultTimeoutMode.SystemSleep }).EffectiveMode(true, false) == VaultTimeoutMode.AppRestart, "unavailable sleep monitor promotes to restart");
        check(!(timed with { Minutes = 0 }).IsValid && !(timed with { Mode = (VaultTimeoutMode)99 }).IsValid, "invalid timeout preferences are rejected");
        var account = AccountSettingsScope.Create(2, "https://EXAMPLE.invalid/", " Test@Example.invalid ");
        check(account == AccountSettingsScope.Create(2, "https://example.invalid", "test@example.invalid"), "account settings normalize server hostname and login email");
        check(account != AccountSettingsScope.Create(2, "https://other.invalid", "test@example.invalid") &&
            account != AccountSettingsScope.Create(2, "https://example.invalid", "other@example.invalid"), "different server and account preferences remain independent");
        check(!account.Contains("example", StringComparison.OrdinalIgnoreCase) && account.Length == 64, "stored account settings keys contain an opaque account identifier");
        var ended = string.Empty;
        void OnEnded(string scope) => ended = scope;
        AccountSettingsScope.SignedOut += OnEnded;
        AccountSettingsScope.Begin(2, "https://example.invalid", "test@example.invalid");
        AccountSettingsScope.End();
        AccountSettingsScope.SignedOut -= OnEnded;
        check(AccountSettingsScope.Current is null && ended == account, "explicit logout publishes the previous account scope and clears active scope");
    }
}
