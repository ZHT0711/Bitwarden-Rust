using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Bytewarden.WinUI.Interop;

internal static class VaultExportChecks
{
    private const string Master = "asdfasdfasdf"; // Public SDK fixture account only.
    private const string ExportPassword = "synthetic portable export password";

    internal static async Task RunAsync(Uri fixture, Action<bool, string> check)
    {
        using var http = new HttpClient { BaseAddress = fixture, Timeout = TimeSpan.FromSeconds(5) };
        var account = RustCore.GetAccounts().Single(item => item.Active).Id;
        var directory = Directory.CreateTempSubdirectory("bytewarden-export-test-");
        var path = Path.Combine(directory.FullName, "合成保险库.json");
        VaultExportReport Export(string format, bool allow = false, string master = Master, string password = "", string? destination = null) =>
            RustCore.ExportVault(account, destination ?? path, format, allow, master, password, CancellationToken.None);
        try
        {
            Reject("validation", () => Export("plainJson"));
            Reject("wrongPassword", () => Export("plainJson", true, master: "invalid public test password"));
            Reject("fileAccess", () => Export("plainJson", true, destination: "relative.json"));
            check(!File.Exists(path), "denied vault exports leave no file");
            try
            {
                RustCore.ExportVault(Guid.NewGuid().ToString(), path, "accountJson", false, Master, "", CancellationToken.None);
                throw new Exception("An inactive account authorized export.");
            }
            catch (OperationCanceledException) { check(!File.Exists(path), "export is bound to the active account"); }

            var report = Export("plainJson", true);
            var raw = await File.ReadAllTextAsync(path);
            using var plain = JsonDocument.Parse(raw);
            var exported = plain.RootElement.GetProperty("items").EnumerateArray().ToArray();
            check(report is { Items: 2, OmittedAttachments: 1, SkippedRestricted: 0 } && !plain.RootElement.GetProperty("encrypted").GetBoolean(),
                "explicit plain JSON export reports the number of items and omitted attachment payloads");
            check(exported.Any(item => item.GetProperty("name").GetString() == "Synthetic protected item" && item.GetProperty("reprompt").GetInt32() == 1) &&
                exported.All(item => item.GetProperty("login").GetProperty("password").ValueKind == JsonValueKind.String &&
                    !item.TryGetProperty("key", out _) && !item.TryGetProperty("attachments", out _)),
                "master-password-authorized export includes protected personal fields but excludes raw cipher and attachment keys");
            Reject("fileAccess", () => Export("accountJson"));
            check(await File.ReadAllTextAsync(path) == raw && directory.GetFiles().Length == 1,
                "export never overwrites an existing file or leaves a temporary copy");
            File.Delete(path);

            check(Export("accountJson").Items == 2, "account-restricted JSON export includes the same personal items");
            var accountEncrypted = await File.ReadAllTextAsync(path);
            using var accountDoc = JsonDocument.Parse(accountEncrypted);
            check(accountDoc.RootElement.GetProperty("encrypted").GetBoolean() &&
                accountDoc.RootElement.GetProperty("encKeyValidation_DO_NOT_EDIT").GetString()!.StartsWith("2.", StringComparison.Ordinal) &&
                accountDoc.RootElement.GetProperty("items").EnumerateArray().All(item => item.GetProperty("name").GetString()!.StartsWith("2.", StringComparison.Ordinal)) &&
                !accountEncrypted.Contains("Synthetic protected item", StringComparison.Ordinal), "account export contains official encrypted fields only");
            File.Delete(path);

            Reject("validation", () => Export("passwordJson"));
            check(Export("passwordJson", password: ExportPassword).Items == 2, "portable JSON export succeeds with a separate password");
            var portable = await File.ReadAllTextAsync(path);
            using var portableDoc = JsonDocument.Parse(portable);
            var envelope = portableDoc.RootElement;
            var derived = Rfc2898DeriveBytes.Pbkdf2(ExportPassword, Encoding.UTF8.GetBytes(envelope.GetProperty("salt").GetString()!),
                envelope.GetProperty("kdfIterations").GetInt32(), HashAlgorithmName.SHA256, 32);
            var encryption = HKDF.Expand(HashAlgorithmName.SHA256, derived, 32, "enc"u8.ToArray());
            var authentication = HKDF.Expand(HashAlgorithmName.SHA256, derived, 32, "mac"u8.ToArray());
            var marker = Decode(envelope.GetProperty("encKeyValidation_DO_NOT_EDIT").GetString()!, encryption, authentication);
            var recovered = Decode(envelope.GetProperty("data").GetString()!, encryption, authentication);
            check(envelope.GetProperty("encrypted").GetBoolean() && envelope.GetProperty("passwordProtected").GetBoolean() &&
               !portable.Contains(ExportPassword, StringComparison.Ordinal) && !portable.Contains("Synthetic protected item", StringComparison.Ordinal) &&
               Guid.TryParse(marker, out _) && recovered == raw, "independent recovery authenticates and decrypts the portable Bitwarden JSON export");
            File.Delete(path);

            await SetPolicy("deny-vault-export");
            Reject("permissionDenied", () => Export("accountJson"));
            Reject("permissionDenied", () => Export("plainJson", true));
            check(!File.Exists(path), "fresh organization policy prevents every format from writing a file despite an earlier allowed sync");
            await SetPolicy("restrict-login-export");
            check(Export("plainJson", true) is { Items: 0, SkippedRestricted: 2 }, "restricted item types are excluded before decrypting backup data");
            File.Delete(path);
            await SetPolicy("clear-export-policy");
            check(RustCore.Lock(), "vault locks before export boundary check");
            Reject("locked", () => Export("accountJson"));
            RustCore.Unlock(Master, CancellationToken.None);
            await CheckImports(raw, accountEncrypted, portable);
        }
        finally { directory.Delete(recursive: true); }

        void Reject(string code, Action action)
        {
            try { action(); throw new Exception($"Expected export error {code}."); }
            catch (VaultException error) { check(error.Code == code, $"export returns {code} (received {error.Code})"); }
        }
        async Task SetPolicy(string policy)
        {
            using var response = await http.PostAsync($"/control/{policy}", null);
            response.EnsureSuccessStatusCode();
        }

        async Task CheckImports(string raw, string accountEncrypted, string portable)
        {
            VaultMutationResult Import(string password = "") => RustCore.ImportVault(account, path, password, CancellationToken.None);
            async Task<int> ImportCalls()
            {
                using var snapshot = JsonDocument.Parse(await http.GetStringAsync("/control/snapshot"));
                return snapshot.RootElement.GetProperty("importRequests").GetInt32();
            }
            await File.WriteAllTextAsync(path, portable);
            Reject("importPassword", () => Import("incorrect synthetic password"));
            check(await ImportCalls() == 0, "invalid backup password performs no server write");
            var originals = RustCore.GetVaultOverview().Items.Select(item => item.Id).ToHashSet();
            var restored = Import(ExportPassword).Overview;
            check(restored.Items.Length == 4 && restored.Items.Count(item => !originals.Contains(item.Id)) == 2 &&
                originals.IsSubsetOf(restored.Items.Select(item => item.Id)), "portable JSON imports new encrypted records without overwriting old items");
            var protectedCopy = restored.Items.Single(item => item.Reprompt && !originals.Contains(item.Id));
            var open = RustCore.GetVaultDetail(protectedCopy.Id, Master, CancellationToken.None);
            check(open.Name == "Synthetic protected item" && open.Attachments == 0, "restored protected item preserves its reprompt and does not invent attachments");

            await File.WriteAllTextAsync(path, accountEncrypted);
            check(Import().Overview.Items.Length == 6, "same-key account JSON import re-encrypts the authenticated backup locally");
            var plain = JsonNode.Parse(raw)!.AsObject();
            var folder = Guid.NewGuid().ToString();
            plain["folders"] = new JsonArray(new JsonObject { ["id"] = folder, ["name"] = "Imported synthetic folder" });
            var rows = plain["items"]!.AsArray();
            foreach (var row in rows) row!["folderId"] = folder;
            await File.WriteAllTextAsync(path, plain.ToJsonString());
            await SetPolicy("restrict-login-export");
            Reject("permissionDenied", () => Import());
            check(await ImportCalls() == 2, "policy-restricted imports fail before posting ciphertext");
            await SetPolicy("clear-export-policy");
            var grouped = Import().Overview;
            var addedFolder = grouped.Folders.Single(item => item.Name == "Imported synthetic folder");
            check(grouped.Items.Length == 8 && grouped.Items.Count(item => item.FolderId == addedFolder.Id) == 2,
                "plain JSON imports both ciphers and their index-bound folder into independently generated IDs");

            rows[0]!["organizationId"] = Guid.NewGuid().ToString();
            await File.WriteAllTextAsync(path, plain.ToJsonString());
            Reject("importFormat", () => Import());
            check(await ImportCalls() == 3, "a personal import never submits organization-scoped files");
            await File.WriteAllTextAsync(path, raw);
            await SetPolicy("ignore-next-import");
            Reject("writeUncertain", () => Import());
            Reject("notSynced", () => Import());
            check(RustCore.SyncVault(CancellationToken.None).Items.Length == 8 && await ImportCalls() == 4,
                "server acknowledgment without the imported ciphertext cannot be mistaken for success");
            await SetPolicy("drop-next-import-response");
            Reject("writeUncertain", () => Import());
            Reject("notSynced", () => Import());
            check(RustCore.SyncVault(CancellationToken.None).Items.Length == 10 && await ImportCalls() == 5,
                "lost import acknowledgment is reconciled by sync without a duplicate retry");
            await SetPolicy("reset");
            check(RustCore.SyncVault(CancellationToken.None).Items.Length == 2, "import fixture cleanup returns the original items for remaining tests");
        }
    }

    private static string Decode(string value, byte[] encryption, byte[] authentication)
    {
        if (!value.StartsWith("2.", StringComparison.Ordinal)) throw new FormatException("Expected an authenticated Bitwarden envelope.");
        var parts = value[2..].Split('|').Select(Convert.FromBase64String).ToArray();
        var mac = HMACSHA256.HashData(authentication.AsSpan(), [.. parts[0], .. parts[1]]);
        if (!CryptographicOperations.FixedTimeEquals(mac, parts[2])) throw new CryptographicException("Export MAC mismatch.");
        using var aes = Aes.Create();
        aes.Key = encryption;
        return Encoding.UTF8.GetString(aes.DecryptCbc(parts[1], parts[0]));
    }
}
