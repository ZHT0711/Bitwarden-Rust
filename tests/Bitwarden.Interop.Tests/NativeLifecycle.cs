using Bitwarden_WinUI.Interop;
using System.Net.Http.Json;
using System.Text.Json;

internal static class NativeLifecycle
{
    private const string FixturePassword = "asdfasdfasdf"; // Public SDK synthetic account, never a real account.

    internal static async Task RunAsync(string address, Action<bool, string> check)
    {
        if (!Uri.TryCreate(address, UriKind.Absolute, out var uri) || !uri.IsLoopback || uri.Scheme != "http")
            throw new ArgumentException("Native lifecycle tests only allow a loopback HTTP fixture server.");
        using var http = new HttpClient { BaseAddress = uri, Timeout = TimeSpan.FromSeconds(10) };
        var health = await http.GetFromJsonAsync<JsonElement>("/health");
        if (!health.GetProperty("fixture").GetBoolean()) throw new Exception("Not a synthetic fixture server.");
        await Control("reset");
        RustCore.Logout();
        var deviceIdentifier = Guid.NewGuid().ToString("D");
        try
        {
            await Control("require-new-device-verification");
            var challenge = RustCore.Login("test@bitwarden.com", FixturePassword, 0, string.Empty, string.Empty, 2, address, deviceIdentifier);
            check(challenge.Status == AuthStatus.NewDeviceVerificationRequired, "native login recognizes new-device email verification instead of reporting a bad password");
            var resend = RustCore.ResendNewDeviceVerification("test@bitwarden.com", FixturePassword, deviceIdentifier, 2, address);
            check(resend == AuthStatus.Authenticated, "native client resends new-device code with SDK-derived master-password hash");
            var invalidCode = RustCore.Login("test@bitwarden.com", FixturePassword, 0, string.Empty, "000000", 2, address, deviceIdentifier);
            check(invalidCode.Status == AuthStatus.InvalidNewDeviceVerificationCode, $"invalid new-device code remains distinct from invalid credentials ({invalidCode.Status})");
            var login = await Task.Run(() => RustCore.Login("test@bitwarden.com", FixturePassword, 0, string.Empty, "246810", 2, address, deviceIdentifier));
            check(login.Status == AuthStatus.Authenticated, "real DLL logs into loopback public SDK fixture");
            check(RustCore.Lock() && RustCore.SessionState == VaultSessionState.Locked, "account can lock before its first sync");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.GetVaultOverview(), "notFound", "unlock before first sync leaves the unsynced vault empty");
            var initial = RustCore.SyncVault(CancellationToken.None);
            check(initial.Items.Length == 2, "real DLL synchronizes encrypted fixture records");
            var draft = new VaultEditorData(null, "ABI synthetic login", 1, null, false, true, null, "synthetic notes",
                new() { ["username"] = "abi-user", ["password"] = "synthetic-abi-secret", ["totp"] = "SYNTHETIC-TOTP" },
                [new("https://example.invalid/abi", 3)], [new("fixture field", "synthetic hidden value", 1, null)],
                true, false, false, false, false);
            var created = RustCore.CreateVaultItem(draft, CancellationToken.None);
            var id = created.Id ?? throw new Exception("Create returned no id.");
            check(created.Overview.Items.Any(item => item.Id == id && item.Name == draft.Name), "create returns server-confirmed list metadata");
            ExpectCode(() => RustCore.GetVaultEditor(id, string.Empty, CancellationToken.None), "passwordRequired", "editor reprompt enforced through ABI");
            var editor = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None);
            check(editor.Values["password"] == "synthetic-abi-secret" && editor.Uris.Single().Match == 3, "SDK-decrypted editor values and URI mode survive ABI");
            var projection = JsonSerializer.Serialize(editor, new JsonSerializerOptions(JsonSerializerDefaults.Web));
            check(!projection.Contains("\"key\":", StringComparison.Ordinal) && !projection.Contains("keyValue", StringComparison.Ordinal) &&
                !projection.Contains("passwordHistory", StringComparison.Ordinal), "editor ABI excludes native keys, passkey material and password history");
            var changed = editor with { Name = "ABI edited login", Values = new(editor.Values) { ["password"] = "synthetic-abi-updated" } };
            var saved = RustCore.UpdateVaultItem(changed, FixturePassword, CancellationToken.None);
            check(saved.Overview.Items.Single(item => item.Id == id).Name == changed.Name, "full SDK edit acknowledged through ABI");
            ExpectCode(() => RustCore.UpdateVaultItem(changed, FixturePassword, CancellationToken.None), "conflict", "stale editor revision cannot overwrite newer local data");
            var latest = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None);
            check(latest.Values["password"] == "synthetic-abi-updated" && latest.CustomFields.Single().Value == "synthetic hidden value", "edit retains hidden custom fields and changed password");

            await Control("fail-next-sync");
            check(RustCore.Lock() && RustCore.SessionState == VaultSessionState.Locked && !RustCore.IsAuthenticated,
                "lock closes native vault access while keeping the account session");
            ExpectCode(() => RustCore.GetVaultOverview(), "locked", "locked overview is inaccessible");
            ExpectCode(() => RustCore.GetVaultDetail(id, FixturePassword, CancellationToken.None), "locked", "item reprompt cannot bypass vault lock");
            ExpectCode(() => RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None), "locked", "locked account cannot open an editor");
            ExpectCode(() => RustCore.SyncVault(CancellationToken.None), "locked", "locked account cannot synchronize");
            ExpectCode(() => RustCore.CreateVaultItem(draft, CancellationToken.None), "locked", "locked account cannot write");
            ExpectCode(() => RustCore.Unlock("wrong synthetic password", CancellationToken.None), "wrongPassword", "incorrect password does not unlock");
            check(RustCore.SessionState == VaultSessionState.Locked, "wrong password leaves native state locked");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            var reopened = RustCore.GetVaultOverview();
            check(RustCore.SessionState == VaultSessionState.Unlocked && reopened.LastSync == initial.LastSync &&
                reopened.Items.Single(item => item.Id == id).Name == changed.Name, "unlock restores the cached vault and confirmed edits without resyncing");
            ExpectCode(() => RustCore.SyncVault(CancellationToken.None), "network", "offline unlock did not consume the pending sync request");
            ExpectCode(() => RustCore.GetVaultEditor(id, string.Empty, CancellationToken.None), "passwordRequired", "unlock preserves item-level reprompt");

            var favored = Action("favorite", string.Empty);
            check(favored.Overview.Items.Single(item => item.Id == id).Favorite, "favorite on reprompt item requires no password");
            var unfavored = Action("unfavorite", string.Empty);
            check(!unfavored.Overview.Items.Single(item => item.Id == id).Favorite, "unfavorite uses partial metadata endpoint");
            var archived = Action("archive", FixturePassword);
            check(archived.Overview.Items.Single(item => item.Id == id).Archived, "archive uses actual native API path");
            var unarchived = Action("unarchive", FixturePassword);
            check(!unarchived.Overview.Items.Single(item => item.Id == id).Archived, "unarchive returns item to active vault");

            var conflictDraft = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None) with { Name = "Uncommitted conflict draft" };
            await Control("conflict-next-write");
            ExpectCode(() => RustCore.UpdateVaultItem(conflictDraft, FixturePassword, CancellationToken.None), "conflict", "official out-of-date HTTP 400 becomes conflict through actual HTTP and ABI");
            ExpectCode(() => RustCore.CreateVaultItem(draft, CancellationToken.None), "notSynced", "server conflict blocks further writes pending reconciliation");
            ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "conflicted cached metadata cannot be presented as a fresh native overview");
            RustCore.SyncVault(CancellationToken.None);
            check(RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None).Name == "ABI edited login", "sync reconciles conflict without applying stale draft");

            var before = await Count();
            await Control("drop-next-write-response");
            var ambiguous = draft with { Name = "ABI uncertain create", Reprompt = false };
            ExpectCode(() => RustCore.CreateVaultItem(ambiguous, CancellationToken.None), "writeUncertain", "lost response after server commit is not reported as a safe retry");
            ExpectCode(() => RustCore.CreateVaultItem(ambiguous, CancellationToken.None), "notSynced", "ambiguous create cannot be retried before sync");
            ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "ambiguous write forces reconciliation even after navigating to a new vault page");
            check(RustCore.Lock(), "vault can lock with an ambiguous write");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "lock and unlock preserve write reconciliation");
            check(await Count() == before + 1, "ambiguous create committed exactly once despite retry attempt");
            var reconciled = RustCore.SyncVault(CancellationToken.None);
            check(reconciled.Items.Count(item => item.Name == ambiguous.Name) == 1, "sync discovers single committed ambiguous create");

            check(Action("trash", FixturePassword).Overview.Items.Single(item => item.Id == id).Deleted, "soft-delete moves item to trash through ABI");
            check(!Action("restore", FixturePassword).Overview.Items.Single(item => item.Id == id).Deleted, "restore returns trashed item through ABI");
            Action("trash", FixturePassword);
            check(Action("delete", FixturePassword).Overview.Items.All(item => item.Id != id), "permanent delete removes server-confirmed item");
            await Control("change-master-password-wrapper");
            RustCore.SyncVault(CancellationToken.None);
            check(RustCore.Lock(), "account can lock again after a complete write lifecycle");
            ExpectCode(() => RustCore.Unlock(FixturePassword, CancellationToken.None), "unlockUnavailable", "changed server wrapper invalidates the cached unlock material");
            RustCore.Logout();
            check(RustCore.SessionState == VaultSessionState.SignedOut, "logout disposes a locked session");
            ExpectCode(() => RustCore.Unlock(FixturePassword, CancellationToken.None), "unlockUnavailable", "logout discards the local unlock material");

            VaultMutationResult Action(string action, string password)
            {
                var item = RustCore.GetVaultOverview().Items.Single(item => item.Id == id);
                return RustCore.VaultAction(id, action, item.RevisionDate, password, CancellationToken.None);
            }
        }
        finally { RustCore.Logout(); await Control("reset"); }
        check(!RustCore.IsAuthenticated, "lifecycle cleanup leaves no authenticated native session");

        async Task Control(string command)
        {
            using var response = await http.PostAsync($"/control/{command}", null);
            response.EnsureSuccessStatusCode();
        }
        async Task<int> Count() => (await http.GetFromJsonAsync<JsonElement>("/control/snapshot")).GetProperty("cipherCount").GetInt32();
        void ExpectCode(Action operation, string expected, string name)
        {
            try { operation(); throw new Exception($"Expected native error {expected}: {name}"); }
            catch (VaultException error) { check(error.Code == expected, name); }
        }
    }
}
