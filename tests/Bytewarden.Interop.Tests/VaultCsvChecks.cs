using System.Text;
using System.Text.Json;
using Bytewarden.WinUI.Interop;
using Microsoft.VisualBasic.FileIO;

internal static class VaultCsvChecks
{
    private const string Master = "asdfasdfasdf"; // Public SDK fixture account only.

    internal static async Task RunAsync(Uri fixture, Action<bool, string> check)
    {
        using var http = new HttpClient { BaseAddress = fixture, Timeout = TimeSpan.FromSeconds(5) };
        var account = RustCore.GetAccounts().Single(item => item.Active).Id;
        var directory = Directory.CreateTempSubdirectory("bytewarden-csv-test-");
        var path = Path.Combine(directory.FullName, "合成保险库.csv");
        VaultMutationResult Import() => RustCore.ImportVault(account, path, "", CancellationToken.None);
        VaultExportReport Export(bool allow = true, string master = Master) =>
            RustCore.ExportVault(account, path, "csv", allow, master, "", CancellationToken.None);
        try
        {
            await Control("reset");
            var initial = RustCore.SyncVault(CancellationToken.None);
            Reject("validation", () => Export(false));
            Reject("wrongPassword", () => Export(master: "wrong synthetic password"));
            check(!File.Exists(path), "CSV requires explicit plaintext consent and valid master-password verification");
            var report = Export();
            var csv = await File.ReadAllTextAsync(path);
            var rows = Parse(csv);
            check(report is { Items: 2, OmittedAttachments: 1, OmittedUnsupported: 0 } && rows.Count == 3 &&
                rows[0].Contains("login_totp") && rows[0].Contains("archivedDate"), "independent CSV reader sees official personal headers and both supported records");
            var name = Array.IndexOf(rows[0], "name");
            var secret = Array.IndexOf(rows[0], "login_password");
            check(rows.Skip(1).Any(row => row[name] == "Synthetic protected item" && row[secret].Length > 0) &&
                !csv.Contains("keyValue", StringComparison.Ordinal), "authorized CSV contains protected login fields but no FIDO2 private material");
            Reject("fileAccess", () => Export());
            check(await File.ReadAllTextAsync(path) == csv && directory.GetFiles().Length == 1, "CSV export never overwrites an existing file or leaves a temporary copy");
            var originalIds = initial.Items.Select(item => item.Id).ToHashSet();
            var restored = Import().Overview;
            check(restored.Items.Length == 4 && originalIds.IsSubsetOf(restored.Items.Select(item => item.Id)), "CSV round trip creates encrypted copies without overwriting source records");
            var protectedCopy = restored.Items.Single(item => item.Reprompt && !originalIds.Contains(item.Id));
            var opened = RustCore.GetVaultEditor(protectedCopy.Id, Master, CancellationToken.None);
            check(opened.Name == "Synthetic protected item" && opened.Reprompt && (opened.Passkeys?.Length ?? 0) == 0,
                "CSV import preserves reprompt while correctly omitting unsupported passkeys");

            await Control("reset");
            RustCore.SyncVault(CancellationToken.None);
            const string noteName = "中文, \"安全笔记\"";
            const string notes = "first\r\nsecond, \"quoted\"";
            const string folderName = "folder,中文";
            var folder = RustCore.CreateFolder(folderName, CancellationToken.None).Id!;
            RustCore.CreateFolder("Empty CSV folder", CancellationToken.None);
            var note = new VaultEditorData(null, noteName, 2, folder, true, true, null, notes,
                new(), [], [new("hidden", " spaces ", 1, null)], true, true, false, false, false);
            var noteId = RustCore.CreateVaultItem(note, CancellationToken.None).Id!;
            var noteRevision = RustCore.GetVaultOverview().Items.Single(item => item.Id == noteId).RevisionDate;
            RustCore.VaultAction(noteId, "archive", noteRevision, Master, CancellationToken.None);
            RustCore.CreateVaultItem(note with { Name = "CSV unsupported card", ItemType = 3, FolderId = null,
                Values = new() { ["number"] = "4111111111111111" }, CustomFields = [] }, CancellationToken.None);
            var trashed = RustCore.CreateVaultItem(note with { Name = "CSV excluded trash", FolderId = null }, CancellationToken.None).Id!;
            var trashRevision = RustCore.GetVaultOverview().Items.Single(item => item.Id == trashed).RevisionDate;
            RustCore.VaultAction(trashed, "trash", trashRevision, Master, CancellationToken.None);
            File.Delete(path);
            report = Export();
            csv = await File.ReadAllTextAsync(path);
            check(report is { Items: 3, Folders: 1, OmittedUnsupported: 1 } &&
                !csv.Contains("CSV unsupported card", StringComparison.Ordinal) && !csv.Contains("CSV excluded trash", StringComparison.Ordinal),
                "CSV reports omitted item types and represented folders, excluding trash and empty folders but retaining archive");
            var before = RustCore.GetVaultOverview().Items.Select(item => item.Id).ToHashSet();
            // Windows spreadsheet tools often prefix UTF-8 CSV with a BOM.
            await File.WriteAllTextAsync(path, csv, new UTF8Encoding(encoderShouldEmitUTF8Identifier: true));
            restored = Import().Overview;
            var noteCopy = restored.Items.Single(item => item.Name == noteName && !before.Contains(item.Id));
            opened = RustCore.GetVaultEditor(noteCopy.Id, Master, CancellationToken.None);
            check(opened.Notes == notes && opened.IsArchived && opened.Favorite && opened.Reprompt &&
                opened.CustomFields.Single() is { Name: "hidden", Value: " spaces ", Type: 0 } &&
                restored.Folders.Single(item => item.Id == noteCopy.FolderId).Name == folderName,
                "actual DLL preserves BOM, UTF-8, quotes, multiline notes, archive, reprompt and folder relations; field types become text");

            await Control("reset");
            RustCore.SyncVault(CancellationToken.None);
            File.Delete(path);
            RustCore.CreateVaultItem(note with { Name = "CSV ambiguous field", FolderId = null,
                CustomFields = [new("field", "key: value", 0, null)], Reprompt = false }, CancellationToken.None);
            Reject("exportCsvField", () => Export());
            check(directory.GetFiles().Length == 0, "unrepresentable CSV field blocks export before writing any plaintext");
            await File.WriteAllTextAsync(path, "type,name,uri,username,password,totp\r\nlogin,legacy,https://example.invalid,u,p,KEY\r\n");
            var legacy = Import().Overview.Items.Single(item => item.Name == "legacy");
            opened = RustCore.GetVaultEditor(legacy.Id, "", CancellationToken.None);
            check(opened.Values["username"] == "u" && opened.Values["password"] == "p" && opened.Values["totp"] == "KEY" &&
                opened.Uris.Single().Uri == "https://example.invalid", "actual DLL accepts official legacy CSV aliases");
            var calls = await ImportCalls();
            foreach (var invalid in new[] {
                "type,name,collections\nlogin,x,shared", "type,name,name\nlogin,a,b", "type,name\ncard,x",
                "type,name,login_password,password\nlogin,x,a,b", "type,name,reprompt\nlogin,x,2", "type,name\nlogin,x,extra" })
            {
                await File.WriteAllTextAsync(path, invalid);
                Reject("importFormat", () => Import());
            }
            check(await ImportCalls() == calls, "malformed, organization and conflicting-secret CSV files perform no bulk server write");
            await File.WriteAllTextAsync(path, "type,name\nlogin,policy");
            await Control("restrict-login-export");
            Reject("permissionDenied", () => Import());
            check(await ImportCalls() == calls, "CSV import checks fresh item-type restrictions before writing");
            File.Delete(path);
            await Control("deny-vault-export");
            Reject("permissionDenied", () => Export());
            check(!File.Exists(path), "CSV export cannot bypass freshly changed organization export policy");
            await Control("clear-export-policy");
            check(RustCore.Lock(), "vault locks before CSV boundary checks");
            Reject("locked", () => Export());
            await File.WriteAllTextAsync(path, "type,name\nlogin,locked");
            Reject("locked", () => Import());
            RustCore.Unlock(Master, CancellationToken.None);
            using var cancelled = new CancellationTokenSource();
            cancelled.Cancel();
            try { RustCore.ImportVault(account, path, "", cancelled.Token); throw new Exception("Cancelled CSV import succeeded."); }
            catch (OperationCanceledException) { check(await ImportCalls() == calls, "pre-cancelled CSV import performs no server write"); }
        }
        finally
        {
            await Control("reset");
            RustCore.SyncVault(CancellationToken.None);
            directory.Delete(recursive: true);
        }

        void Reject(string code, Action action)
        {
            try { action(); throw new Exception($"Expected CSV error {code}."); }
            catch (VaultException error) { check(error.Code == code, $"CSV returns {code} (received {error.Code})"); }
        }
        async Task Control(string action)
        {
            using var response = await http.PostAsync($"/control/{action}", null);
            response.EnsureSuccessStatusCode();
        }
        async Task<int> ImportCalls()
        {
            using var snapshot = JsonDocument.Parse(await http.GetStringAsync("/control/snapshot"));
            return snapshot.RootElement.GetProperty("importRequests").GetInt32();
        }
    }

    private static List<string[]> Parse(string text)
    {
        using var parser = new TextFieldParser(new StringReader(text)) { HasFieldsEnclosedInQuotes = true, TrimWhiteSpace = false };
        parser.SetDelimiters(",");
        var rows = new List<string[]>();
        while (!parser.EndOfData) rows.Add(parser.ReadFields()!);
        return rows;
    }
}
