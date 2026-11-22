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
        var health = await http.GetFromJsonAsync("/health", FixtureJsonContext.Default.JsonElement);
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
            check(secondFactor.Status == AuthStatus.TwoFactorRequired && secondFactor.AvailableTwoFactorMethods == 1,
                "nested self-hosted authenticator challenge preserves the actual provider");
            var malformed = RustCore.Login("test@bitwarden.com", FixturePassword, 0, "abc", "", 2, address, deviceIdentifier);
            check(malformed.Status == AuthStatus.InvalidTwoFactorCode && !RustCore.IsAuthenticated,
                "malformed authenticator codes cannot authenticate");
            var rejected = RustCore.Login("test@bitwarden.com", FixturePassword, 0, "123456", "", 2, address, deviceIdentifier);
            check(rejected.Status == AuthStatus.InvalidTwoFactorCode && !RustCore.IsAuthenticated,
                "a self-hosted server's invalid or replayed authenticator token is not reported as a bad master password");
            var login = await Task.Run(() => RustCore.Login("test@bitwarden.com", FixturePassword, 0, "087 349", "", 2, address, deviceIdentifier));
            check(login.Status == AuthStatus.Authenticated, "real DLL accepts a new authenticator code with whitespace and a leading zero after a rejected attempt");
            check(RustCore.Lock() && RustCore.SessionState == VaultSessionState.Locked, "account can lock before its first sync");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.GetVaultOverview(), "notFound", "unlock before first sync leaves the unsynced vault empty");
            var initial = RustCore.SyncVault(CancellationToken.None);
            check(initial.Items.Length == 2, "real DLL syncs a profile with wrapped account keys without a host state bridge");
            await SendChecks.RunAsync(uri, check);
            await VaultExportChecks.RunAsync(uri, check);
            var protectedItem = initial.Items.Single(item => item.Name == "Synthetic protected item");
            ExpectCode(() => RustCore.GetVaultDetail(protectedItem.Id, "", CancellationToken.None), "passwordRequired",
                "an attachment detail lease requires the item's master-password reprompt");
            var protectedDetail = RustCore.GetVaultDetail(protectedItem.Id, FixturePassword, CancellationToken.None);
            var attachment = protectedDetail.AttachmentFiles!.Single();
            var attachmentRequest = protectedDetail.AttachmentRequestId!.Value;
            var summary = JsonSerializer.Serialize(attachment, FixtureJsonContext.Default.VaultAttachment);
            check(attachment.Name == "synthetic-attachment.txt" && attachment.Size > 0 &&
                !summary.Contains("\"key\"", StringComparison.Ordinal) && !summary.Contains("\"url\"", StringComparison.Ordinal),
                "attachment details expose the decrypted name and size without keys or download URLs");
            var directory = Directory.CreateTempSubdirectory("bitwarden-attachment-test-");
            var destination = Path.Combine(directory.FullName, "合成附件.txt");
            const string fixtureContents = "Bitwarden synthetic attachment fixture\n";
            try
            {
                File.WriteAllText(destination, "existing file");
                RustCore.DownloadVaultAttachment(attachment.Id, destination, attachmentRequest, CancellationToken.None);
                check(File.ReadAllText(destination) == fixtureContents, "the native attachment download saves exact authenticated plaintext");

                await Control("tamper-next-attachment");
                ExpectCode(() => RustCore.DownloadVaultAttachment(attachment.Id, destination, attachmentRequest, CancellationToken.None),
                    "decryptionFailed", "tampered attachment bytes are rejected");
                check(File.ReadAllText(destination) == fixtureContents && directory.GetFiles().Length == 1,
                    "failed decryption retains the original destination and leaves no temporary file");

                var downloadsBefore = await FixtureCount("attachmentDownloads");
                await Control("pause-next-attachment");
                using var cancellation = new CancellationTokenSource(TimeSpan.FromSeconds(10));
                var pending = Task.Run(() => RustCore.DownloadVaultAttachment(attachment.Id, destination, attachmentRequest, cancellation.Token));
                var live = await WaitForTransfer("attachmentDownloads", downloadsBefore, pending);
                cancellation.Cancel();
                try { await pending; throw new Exception("Cancelled download completed unexpectedly."); }
                catch (OperationCanceledException)
                {
                    check(live && File.ReadAllText(destination) == fixtureContents && directory.GetFiles().Length == 1,
                        "cancelling a live transfer retains the original destination and leaves no temporary file");
                }
                ExpectCode(() => RustCore.DownloadVaultAttachment(attachment.Id, destination, attachmentRequest, CancellationToken.None),
                    "attachmentViewExpired", "an expired detail lease cannot authorize another attachment transfer");
                check(RustCore.Lock(), "vault locks after cancelling attachment activity");
                ExpectCode(() => RustCore.DownloadVaultAttachment(attachment.Id, destination, attachmentRequest, CancellationToken.None),
                    "locked", "locked vault refuses attachment downloads");
                RustCore.Unlock(FixturePassword, CancellationToken.None);
            }
            finally
            {
                File.Delete(destination);
                directory.Delete();
            }
            var uploadDirectory = Directory.CreateTempSubdirectory("bitwarden-upload-test-");
            var source = Path.Combine(uploadDirectory.FullName, "合成上传.txt");
            var received = Path.Combine(uploadDirectory.FullName, "合成下载.txt");
            var editableId = initial.Items.Single(item => item.Name == "Synthetic login").Id;
            VaultDetail OpenEditable() => RustCore.GetVaultDetail(editableId, "", CancellationToken.None);
            try
            {
                File.WriteAllText(source, fixtureContents);
                var editable = OpenEditable();
                check(editable.Attachments == 0 && editable.CanUploadAttachments && editable.CanDeleteAttachments,
                    "editable item without attachments grants an authorized upload");
                RustCore.UploadVaultAttachment(source, editable.AttachmentRequestId!.Value, CancellationToken.None);
                editable = OpenEditable();
                var uploaded = editable.AttachmentFiles!.Single();
                check(uploaded.Name == Path.GetFileName(source) && uploaded.CanDownload &&
                    editable.Passkeys?.Length == 1, "direct upload preserves the encrypted filename and existing passkey");
                RustCore.DownloadVaultAttachment(uploaded.Id, received, editable.AttachmentRequestId!.Value, CancellationToken.None);
                check(File.ReadAllText(received) == fixtureContents && File.ReadAllText(source) == fixtureContents,
                    "the encrypted direct upload round-trips without modifying the source");

                await Control("azure-next-attachment-upload");
                RustCore.UploadVaultAttachment(source, editable.AttachmentRequestId!.Value, CancellationToken.None);
                editable = OpenEditable();
                var azure = editable.AttachmentFiles!.Single(file => file.Id != uploaded.Id);
                RustCore.DownloadVaultAttachment(azure.Id, received, editable.AttachmentRequestId!.Value, CancellationToken.None);
                check(File.ReadAllText(received) == fixtureContents, "presigned upload round-trips without sending the vault token");
                RustCore.DeleteVaultAttachment(azure.Id, editable.AttachmentRequestId!.Value, CancellationToken.None);
                RustCore.SyncVault(CancellationToken.None);
                editable = OpenEditable();
                check(editable.AttachmentFiles!.Single().Id == uploaded.Id, "delete removes only the selected attachment after sync");

                await Control("fail-next-attachment-upload");
                ExpectCode(() => RustCore.UploadVaultAttachment(source, editable.AttachmentRequestId!.Value, CancellationToken.None),
                    "attachmentUploadFailed", "an upload failure rolls back the newly created attachment");
                check(RustCore.GetVaultOverview().Items.Any(item => item.Id == editableId),
                    "confirmed rollback leaves the vault ready for use");
                RustCore.SyncVault(CancellationToken.None);
                editable = OpenEditable();
                check(editable.AttachmentFiles!.Single().Id == uploaded.Id, "upload rollback keeps every pre-existing attachment");

                var uploadsBefore = await FixtureCount("attachmentUploads");
                await Control("pause-next-attachment-upload");
                using (var cancelUpload = new CancellationTokenSource(TimeSpan.FromSeconds(10)))
                {
                    var pending = Task.Run(() => RustCore.UploadVaultAttachment(
                        source, editable.AttachmentRequestId!.Value, cancelUpload.Token));
                    var live = await WaitForTransfer("attachmentUploads", uploadsBefore, pending);
                    cancelUpload.Cancel();
                    try { await pending; throw new Exception("Cancelled upload completed unexpectedly."); }
                    catch (OperationCanceledException) { check(live, "live upload cancellation waits for slot cleanup"); }
                }
                RustCore.SyncVault(CancellationToken.None);
                editable = OpenEditable();
                check(editable.AttachmentFiles!.Single().Id == uploaded.Id, "cancelled upload removes the new data without removing any prior attachment");

                await Control("fail-next-attachment-upload");
                await Control("fail-next-attachment-delete");
                ExpectCode(() => RustCore.UploadVaultAttachment(source, editable.AttachmentRequestId!.Value, CancellationToken.None),
                    "writeUncertain", "unconfirmed cleanup blocks another write pending sync");
                ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "unconfirmed attachment cleanup invalidates the overview");
                RustCore.SyncVault(CancellationToken.None);
                editable = OpenEditable();
                var orphan = editable.AttachmentFiles!.Single(file => file.Id != uploaded.Id);
                RustCore.DeleteVaultAttachment(orphan.Id, editable.AttachmentRequestId!.Value, CancellationToken.None);
                editable = OpenEditable();
                RustCore.DeleteVaultAttachment(uploaded.Id, editable.AttachmentRequestId!.Value, CancellationToken.None);
                check(OpenEditable().Attachments == 0, "deleting the last attachment leaves the item available");
            }
            finally
            {
                File.Delete(source);
                File.Delete(received);
                uploadDirectory.Delete();
            }
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
            check(!JsonSerializer.Serialize(detail, FixtureJsonContext.Default.VaultDetail).Contains(seed, StringComparison.Ordinal), "detail ABI keeps the TOTP seed in Rust");
            check(RustCore.RefreshTotp(requestForTotp).Code.Length == 8, "a verified item refreshes its code without another password");
            RustCore.CloseVaultDetail(requestForTotp);
            ExpectCode(() => RustCore.RefreshTotp(requestForTotp), "totpViewExpired", "closing details revokes code generation");
            ExpectCode(() => RustCore.GetVaultEditor(id, string.Empty, CancellationToken.None), "passwordRequired", "editor reprompt enforced through ABI");
            var editor = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None);
            check(editor.Values["password"] == "synthetic-abi-secret" && editor.Uris.Single().Match == 3, "SDK-decrypted editor values and URI mode survive ABI");
            var projection = RustCore.JsonForChecks(editor);
            check(!projection.Contains("\"key\":", StringComparison.Ordinal) && !projection.Contains("keyValue", StringComparison.Ordinal) &&
                !projection.Contains("passwordHistory", StringComparison.Ordinal), "editor ABI excludes native keys, passkey material and password history");
            var changed = editor with { Name = "ABI edited login", Values = new(editor.Values) { ["password"] = "synthetic-abi-updated" } };
            var saved = RustCore.UpdateVaultItem(changed, FixturePassword, CancellationToken.None);
            check(saved.Overview.Items.Single(item => item.Id == id).Name == changed.Name, "full SDK edit acknowledged through ABI");
            ExpectCode(() => RustCore.UpdateVaultItem(changed, FixturePassword, CancellationToken.None), "conflict", "stale editor revision cannot overwrite newer local data");
            var latest = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None);
            check(latest.Values["password"] == "synthetic-abi-updated" && latest.CustomFields.Single().Value == "synthetic hidden value", "edit retains hidden custom fields and changed password");

            var createdFolder = RustCore.CreateFolder("Synthetic folder", CancellationToken.None);
            var folderId = createdFolder.Id ?? throw new Exception("Folder create returned no id.");
            check(createdFolder.Overview.Folders.Any(folder => folder.Id == folderId && folder.Name == "Synthetic folder"),
                "folder creation returns server-confirmed decrypted metadata");
            var renamedFolder = RustCore.RenameFolder(folderId, "Renamed synthetic folder", CancellationToken.None);
            check(renamedFolder.Overview.Folders.Any(folder => folder.Id == folderId && folder.Name == "Renamed synthetic folder"),
                "folder rename updates SDK repository and overview");
            var assigned = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None) with { FolderId = folderId };
            var assignment = RustCore.UpdateVaultItem(assigned, FixturePassword, CancellationToken.None);
            check(assignment.Overview.Items.Single(item => item.Id == id).FolderId == folderId,
                "an item can be assigned to a newly created folder");
            var deletedFolder = RustCore.DeleteFolders([folderId], CancellationToken.None);
            check(!deletedFolder.Overview.Folders.Any(folder => folder.Id == folderId) &&
                deletedFolder.Overview.Items.Single(item => item.Id == id) is { FolderId: null, Name: "ABI edited login" },
                "folder deletion keeps its item and clears its folder assignment");
            var reconciledFolderDelete = RustCore.SyncVault(CancellationToken.None);
            check(!reconciledFolderDelete.Folders.Any(folder => folder.Id == folderId) &&
                reconciledFolderDelete.Items.Single(item => item.Id == id).FolderId is null,
                "a full server sync confirms the deleted folder and retained unassigned item");

            string CreateFolderId(string name) =>
                RustCore.CreateFolder(name, CancellationToken.None).Id ?? throw new Exception("Folder create returned no id.");
            var a = CreateFolderId("Synthetic batch A");
            var b = CreateFolderId("Synthetic batch B");
            var retainedFolder = CreateFolderId("Synthetic batch retained");
            var batchItem = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None) with { FolderId = a };
            RustCore.UpdateVaultItem(batchItem, FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.DeleteFolders([a, a], CancellationToken.None), "validation", "duplicate folder IDs are rejected before deleting");
            var batchDelete = RustCore.DeleteFolders([a, b], CancellationToken.None);
            check(batchDelete.Overview.Folders.All(folder => folder.Id != a && folder.Id != b) &&
                batchDelete.Overview.Folders.Any(folder => folder.Id == retainedFolder) &&
                batchDelete.Overview.Items.Single(item => item.Id == id).FolderId is null &&
                RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None).FolderId is null,
                "bulk delete clears the assigned folder in both item metadata and the encrypted cache");

            var otherItem = batchDelete.Overview.Items.Single(item => item.Name == "Synthetic login").Id;
            var movedIds = new[] { id, otherItem };
            var moved = RustCore.MoveVaultItems(movedIds, retainedFolder, CancellationToken.None);
            check(moved.Overview.Items.Where(item => movedIds.Contains(item.Id))
                .All(item => item.FolderId == retainedFolder),
                "bulk move changes personal folder metadata without requiring an item password");
            var movedEditor = RustCore.GetVaultEditor(id, FixturePassword, CancellationToken.None);
            check(movedEditor.FolderId == retainedFolder && movedEditor.Values["password"] == "synthetic-abi-updated"
                && movedEditor.Reprompt && movedEditor.CustomFields.Single().Value == "synthetic hidden value",
                "moving a protected item preserves the secret fields and reprompt");
            await Control("fail-next-move-after-first");
            ExpectCode(() => RustCore.MoveVaultItems(movedIds, null, CancellationToken.None), "writeUncertain",
                "a server error after a partial item move requires reconciliation");
            ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "partial move blocks stale overview reads");
            var reconciledMove = RustCore.SyncVault(CancellationToken.None);
            check(reconciledMove.Items.Single(item => item.Id == id).FolderId is null &&
                reconciledMove.Items.Single(item => item.Id == otherItem).FolderId == retainedFolder,
                "sync discovers which item was moved before the server error");
            RustCore.MoveVaultItems(movedIds, null, CancellationToken.None);
            check(RustCore.SyncVault(CancellationToken.None).Items.Where(item => movedIds.Contains(item.Id))
                .All(item => item.FolderId is null),
                "a full sync confirms the move to no folder");

            var first = CreateFolderId("Synthetic fallback first");
            var second = CreateFolderId("Synthetic fallback second");
            await Control("reject-folder-batch");
            await Control("fail-second-folder-delete");
            ExpectCode(() => RustCore.DeleteFolders([first, second], CancellationToken.None), "writeUncertain",
                "partial deletion on a server without bulk support requires reconciliation");
            ExpectCode(() => RustCore.GetVaultOverview(), "notSynced", "partial folder delete blocks stale overview reads");
            var reconciledFolders = RustCore.SyncVault(CancellationToken.None);
            check(reconciledFolders.Folders.All(folder => folder.Id != first) &&
                reconciledFolders.Folders.Any(folder => folder.Id == second),
                "sync reconciles the individually confirmed deletion after a partial failure");
            await Control("reject-folder-batch");
            var fallback = RustCore.DeleteFolders([second, retainedFolder], CancellationToken.None);
            check(fallback.Overview.Folders.All(folder => folder.Id != second && folder.Id != retainedFolder),
                "individual fallback deletes all selected folders when the bulk route is unavailable");
            var lastConfirmedSync = RustCore.SyncVault(CancellationToken.None).LastSync;

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
            ExpectCode(() => RustCore.CreateFolder("Locked synthetic folder", CancellationToken.None), "locked",
                "locked account cannot create folders");
            ExpectCode(() => RustCore.Unlock("wrong synthetic password", CancellationToken.None), "wrongPassword", "incorrect password does not unlock");
            check(RustCore.SessionState == VaultSessionState.Locked, "wrong password leaves native state locked");
            RustCore.Unlock(FixturePassword, CancellationToken.None);
            ExpectCode(() => RustCore.RefreshTotp(totpBeforeLock), "totpViewExpired", "unlock does not reauthorize a dismissed detail");
            var reopened = RustCore.GetVaultOverview();
            check(RustCore.SessionState == VaultSessionState.Unlocked && reopened.LastSync == lastConfirmedSync &&
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
            var passkeyDetailJson = JsonSerializer.Serialize(passkeyDetail, FixtureJsonContext.Default.VaultDetail);
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
        if (health.TryGetProperty("peerPort", out var peerPort))
            await CheckAccountSwitching(new UriBuilder(uri) { Port = peerPort.GetInt32() }.Uri);

        async Task CheckAccountSwitching(Uri peer)
        {
            using var otherServer = new HttpClient { BaseAddress = peer, Timeout = TimeSpan.FromSeconds(10) };
            var otherHealth = await otherServer.GetFromJsonAsync("/health", FixtureJsonContext.Default.JsonElement);
            if (!otherHealth.GetProperty("fixture").GetBoolean()) throw new Exception("Second server must be a loopback fixture.");
            var otherAddress = peer.GetLeftPart(UriPartial.Authority);
            var device = Guid.NewGuid().ToString("D");
            AuthStatus LoginAt(string server, string email = "test@bitwarden.com") =>
                RustCore.Login(email, FixturePassword, 0, "", "", 2, server, device).Status;
            bool HasItem(string name) => RustCore.GetVaultOverview().Items.Any(item => item.Name == name);
            void CreateNote(string note) => RustCore.CreateVaultItem(new VaultEditorData(
                null, note, 2, null, false, false, null, "Synthetic fixture note", [], [], [], true, false, false, false, false),
                CancellationToken.None);

            try
            {
                check(LoginAt(address) == AuthStatus.Authenticated, "primary account logs in");
                var first = RustCore.GetAccounts().Single();
                RustCore.SyncVault(CancellationToken.None);
                CreateNote("Primary account item");
                check(LoginAt(address, " TEST@BITWARDEN.COM ") == AuthStatus.AccountAlreadySignedIn,
                    "a duplicate account is rejected on the same server");
                check(LoginAt(otherAddress, "unregistered@example.invalid") == AuthStatus.Failed &&
                    RustCore.GetAccounts().Single().Id == first.Id && HasItem("Primary account item"),
                    "a failed second login preserves the active account and cache");

                check(LoginAt(otherAddress) == AuthStatus.Authenticated, "the same email on another server logs in independently");
                var second = RustCore.GetAccounts().Single(account => account.Active);
                check(first.Id != second.Id && second.ServerUrl == otherAddress &&
                    RustCore.GetAccounts().Single(account => account.Id == first.Id).State == VaultSessionState.Unlocked,
                    "switching identities retains the previous account's server and lock state");
                RustCore.SyncVault(CancellationToken.None);
                CreateNote("Secondary account item");
                check(HasItem("Secondary account item") && !HasItem("Primary account item"),
                    "secondary account uses its own authorized server and item cache");
                RustCore.SwitchAccount(first.Id);
                check(HasItem("Primary account item") && !HasItem("Secondary account item"),
                    "switching back restores the first cached vault without server I/O");
                var item = RustCore.GetVaultOverview().Items.Single(candidate => candidate.Name == "Synthetic login");
                var detail = RustCore.GetVaultDetail(item.Id, "", CancellationToken.None);

                var syncs = await FixtureCount("syncRequests");
                await Control("pause-next-sync");
                var slowSync = Task.Run(() => RustCore.SyncVault(CancellationToken.None));
                var live = await WaitForTransfer("syncRequests", syncs, slowSync);
                RustCore.SwitchAccount(second.Id);
                try { await slowSync; throw new Exception("Expected the old account's synchronization to be cancelled."); }
                catch (OperationCanceledException)
                {
                    check(live && HasItem("Secondary account item"), "switching cancels a live sync on the old server");
                }
                if (detail.TotpRequestId is { } request)
                    ExpectCode(() => RustCore.RefreshTotp(request), "totpViewExpired", "switching revokes old item access");

                var timedOut = RustCore.ApplyAccountTimeouts([new(first.Id, false)], interrupt: false);
                check(timedOut.Single(account => account.Id == first.Id).State == VaultSessionState.Locked &&
                    timedOut.Single(account => account.Active).Id == second.Id && RustCore.IsAuthenticated,
                    "an inactive account can time out independently of the active vault");
                RustCore.SwitchAccount(first.Id);
                check(RustCore.SessionState == VaultSessionState.Locked, "switching to a locked account preserves its lock");
                ExpectCode(() => RustCore.GetVaultOverview(), "locked", "a locked account cannot reveal cached metadata");
                await Control("fail-next-sync");
                RustCore.Unlock(FixturePassword, CancellationToken.None);
                check(HasItem("Primary account item"), "the locked account restores its own cached data while its server is unavailable");
                ExpectCode(() => RustCore.SyncVault(CancellationToken.None), "network", "offline unlock does not depend on a sync");
                RustCore.Logout();
                check(RustCore.GetAccounts().Single().Id == second.Id && HasItem("Secondary account item"),
                    "logging out the active account selects the remaining account");
            }
            finally
            {
                RustCore.LogoutAll();
                await Control("reset");
                using var reset = await otherServer.PostAsync("/control/reset", null);
                reset.EnsureSuccessStatusCode();
            }
            check(RustCore.GetAccounts().Length == 0 && RustCore.SessionState == VaultSessionState.SignedOut,
                "closing the session removes all account clients");
        }

        async Task Control(string command)
        {
            using var response = await http.PostAsync($"/control/{command}", null);
            response.EnsureSuccessStatusCode();
        }
        Task<int> Count() => FixtureCount("cipherCount");
        async Task<int> FixtureCount(string property) =>
            (await http.GetFromJsonAsync("/control/snapshot", FixtureJsonContext.Default.JsonElement)).GetProperty(property).GetInt32();
        async Task<bool> WaitForTransfer(string counter, int before, Task pending)
        {
            var deadline = DateTime.UtcNow.AddSeconds(2);
            do
            {
                if (await FixtureCount(counter) > before) return !pending.IsCompleted;
                await Task.Delay(20);
            } while (DateTime.UtcNow < deadline && !pending.IsCompleted);
            return false;
        }
        void ExpectCode(Action operation, string expected, string name)
        {
            try { operation(); throw new Exception($"Expected native error {expected}: {name}"); }
            catch (VaultException error) { check(error.Code == expected, name); }
        }
    }
}
