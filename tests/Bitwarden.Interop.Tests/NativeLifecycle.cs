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
        check(string.IsNullOrEmpty(RustCore.PendingPasskey().Id), "native passkey ABI has no unsigned pending operation");
        RustCore.CancelPasskey("forged-synthetic-request");
        check(!RustCore.IsPasskeyActive("forged-synthetic-request"), "arbitrary UI identifiers cannot create a passkey request");
        try { RustCore.RespondPasskey("forged-synthetic-request", null, "", 0); throw new Exception("Unsigned passkey request accepted."); }
        catch (OperationCanceledException) { check(true, "passkey approval requires a current Windows-signed operation"); }
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
            await Control("require-authenticator");
            var secondFactor = RustCore.Login("test@bitwarden.com", FixturePassword, 0, string.Empty, "246810", 2, address, deviceIdentifier);
            check(secondFactor.Status == AuthStatus.TwoFactorRequired && (secondFactor.AvailableTwoFactorMethods & 1) != 0,
                "NodeWarden null authenticator challenge keeps its verification method");
            var login = await Task.Run(() => RustCore.Login("test@bitwarden.com", FixturePassword, 0, "287349", "246810", 2, address, deviceIdentifier));
            check(login.Status == AuthStatus.Authenticated, "real DLL handles equivalent NodeWarden casing aliases during prelogin and token exchange");
            check(RustCore.Lock() && RustCore.SessionState == VaultSessionState.Locked, "account can lock before its first sync");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.GetVaultOverview(), "notFound", "unlock before first sync leaves the unsynced vault empty");
            var initial = RustCore.SyncVault(CancellationToken.None);
            check(initial.Items.Length == 2, "real DLL syncs a profile with wrapped account keys without a host state bridge");
            const string seed = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
            var draft = new VaultEditorData(null, "ABI synthetic login", 1, null, false, true, null, "synthetic notes",
                new() { ["username"] = "abi-user", ["password"] = "synthetic-abi-secret", ["totp"] = $"otpauth://totp/Synthetic?secret={seed}&digits=8&period=17" },
                [new("https://example.com/abi?fixture=synthetic", 3)], [new("fixture field", "synthetic hidden value", 1, null)],
                true, false, false, false, false);
            var created = RustCore.CreateVaultItem(draft, CancellationToken.None);
            var id = created.Id ?? throw new Exception("Create returned no id.");
            check(created.Overview.Items.Any(item => item.Id == id && item.Name == draft.Name), "create returns server-confirmed list metadata");
            check(created.Overview.Items.Single(item => item.Id == id).WebsiteHost == "example.com",
                "icon projection exposes only the validated hostname");
            ExpectCode(() => RustCore.GetVaultDetail(id, string.Empty, CancellationToken.None), "passwordRequired", "TOTP requires item reprompt");
            var detail = RustCore.GetVaultDetail(id, FixturePassword, CancellationToken.None);
            var requestForTotp = detail.TotpRequestId ?? throw new Exception("TOTP access was not granted.");
            check(detail.Totp is { Status: "ready", Value: { Period: 17 } totp }
                && totp.Code.Length == 8 && totp.ValidUntil - totp.ValidFrom == 17, "details contain the generated code and custom period");
            check(!JsonSerializer.Serialize(detail).Contains(seed, StringComparison.Ordinal), "detail ABI keeps the TOTP seed in Rust");
            check(RustCore.RefreshTotp(requestForTotp).Code.Length == 8, "a verified item refreshes its code without another password");
            RustCore.CloseTotp(requestForTotp);
            ExpectCode(() => RustCore.RefreshTotp(requestForTotp), "totpViewExpired", "closing details revokes code generation");
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

            var totpBeforeLock = RustCore.GetVaultDetail(id, FixturePassword, CancellationToken.None).TotpRequestId!.Value;
            await Control("fail-next-sync");
            check(RustCore.Lock() && RustCore.SessionState == VaultSessionState.Locked && !RustCore.IsAuthenticated,
                "lock closes native vault access while keeping the account session");
            ExpectCode(() => RustCore.GetVaultOverview(), "locked", "locked overview is inaccessible");
            ExpectCode(() => RustCore.RefreshTotp(totpBeforeLock), "locked", "lock revokes live TOTP access");
            ExpectCode(() => RustCore.GetVaultDetail(id, FixturePassword, CancellationToken.None), "locked", "item reprompt cannot bypass vault lock");
            ExpectCode(() => RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None), "locked", "locked account cannot open an editor");
            ExpectCode(() => RustCore.SyncVault(CancellationToken.None), "locked", "locked account cannot synchronize");
            ExpectCode(() => RustCore.CreateVaultItem(draft, CancellationToken.None), "locked", "locked account cannot write");
            ExpectCode(() => RustCore.Unlock("wrong synthetic password", CancellationToken.None), "wrongPassword", "incorrect password does not unlock");
            check(RustCore.SessionState == VaultSessionState.Locked, "wrong password leaves native state locked");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.RefreshTotp(totpBeforeLock), "totpViewExpired", "unlock does not reauthorize a dismissed detail");
            var reopened = RustCore.GetVaultOverview();
            check(RustCore.SessionState == VaultSessionState.Unlocked && reopened.LastSync == initial.LastSync &&
                reopened.Items.Single(item => item.Id == id).Name == changed.Name, "unlock restores the cached vault and confirmed edits without resyncing");
            ExpectCode(() => RustCore.SyncVault(CancellationToken.None), "network", "offline unlock did not consume the pending sync request");
            ExpectCode(() => RustCore.GetVaultEditor(id, string.Empty, CancellationToken.None), "passwordRequired", "unlock preserves item-level reprompt");
            await Control("disable-premium");
            RustCore.SyncVault(CancellationToken.None);
            var freeDetail = RustCore.GetVaultDetail(id, FixturePassword, CancellationToken.None);
            check(freeDetail.Totp is { Status: "premiumRequired", Value: null } && freeDetail.TotpRequestId is null,
                "a free account can store its key while code generation requires its entitlement");
            await Control("enable-premium");
            RustCore.SyncVault(CancellationToken.None);

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

            var passkeyId = RustCore.GetVaultOverview().Items.Single(item => item.Name == "Synthetic login").Id;
            var passkeyDetail = RustCore.GetVaultDetail(passkeyId, string.Empty, CancellationToken.None);
            check(passkeyDetail.Passkeys is [{ RelyingParty: "example.com", UserName: "fixture-user" }],
                "detail exposes the saved passkey's display metadata");
            var passkeyDetailJson = JsonSerializer.Serialize(passkeyDetail);
            check(!passkeyDetailJson.Contains("SYNTHETIC-PASSKEY-PRIVATE", StringComparison.Ordinal) &&
                !passkeyDetailJson.Contains("SYNTHETIC-SITE-HANDLE", StringComparison.Ordinal) &&
                !passkeyDetailJson.Contains("SYNTHETIC-CREDENTIAL", StringComparison.Ordinal),
                "passkey detail never exposes the private key, credential id or user handle");
            var passkeyEditor = RustCore.GetVaultEditor(passkeyId, string.Empty, CancellationToken.None);
            RustCore.UpdateVaultItem(passkeyEditor with { Notes = "synthetic edit keeps passkey" }, string.Empty, CancellationToken.None);
            var retained = RustCore.GetVaultEditor(passkeyId, string.Empty, CancellationToken.None);
            check(retained.Passkeys?.Length == 1, "normal edit keeps the existing passkey");
            RustCore.UpdateVaultItem(retained with { RemoveSavedPasskey = true }, string.Empty, CancellationToken.None);
            RustCore.SyncVault(CancellationToken.None);
            var removed = RustCore.GetVaultDetail(passkeyId, string.Empty, CancellationToken.None);
            check(removed.Passkeys?.Length == 0 && removed.Totp is { Status: "ready" },
                "saving passkey removal persists through sync and preserves the login TOTP");

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
