using Bitwarden_WinUI.Interop;
using System.Net.Http.Json;

internal static class NativeLinkedFieldChecks
{
    private const string Password = "asdfasdfasdf"; // Public SDK synthetic account only.

    internal static async Task RunAsync(string address, string cacheRoot, Action<bool, string> check)
    {
        if (!Uri.TryCreate(address, UriKind.Absolute, out var uri) || !uri.IsLoopback || uri.Scheme != "http")
            throw new ArgumentException("Linked-field checks require a loopback HTTP fixture server.");
        using var http = new HttpClient { BaseAddress = uri, Timeout = TimeSpan.FromSeconds(5) };
        var health = await http.GetFromJsonAsync("/health", FixtureJsonContext.Default.JsonElement);
        if (!health.GetProperty("fixture").GetBoolean()) throw new InvalidOperationException("Not a synthetic fixture server.");
        if (!Directory.Exists(cacheRoot) || Directory.EnumerateFileSystemEntries(cacheRoot).Any())
            throw new ArgumentException("Linked-field checks require a new empty cache directory owned by the test host.");
        try
        {
            check(RustCore.ConfigureLocalCache(cacheRoot), "linked checks isolate native cache from real accounts");
            await Control("reset");
            var login = RustCore.Login("test@bitwarden.com", Password, 0, "", "", 2, address, Guid.NewGuid().ToString("D"));
            check(login.Status == AuthStatus.Authenticated, "linked checks log in only to the public synthetic fixture");
            var overview = RustCore.SyncVault(CancellationToken.None);
            check(overview.Items.Length == 6 && overview.Failures == 0, "linked fixture decrypts six healthy SDK ciphers");
            check(!RustCore.JsonForChecks(overview).Contains("linked-101", StringComparison.Ordinal) &&
                !RustCore.JsonForChecks(overview).Contains("synthetic-password-DO-NOT-USE", StringComparison.Ordinal),
                "linked values do not enlarge or leak into the list overview");
            var syncs = await Syncs();
            foreach (var (name, expected) in new (string, (int Id, string Value, bool Sensitive)[])[]
            {
                ("Synthetic login", [(100, "fixture-user", false), (101, "synthetic-password-DO-NOT-USE", true)]),
                ("Synthetic linked card", [
                    (300, "Public Cardholder", false), (301, "09", false), (302, "2031", false),
                    (303, "synthetic-security-code", true), (304, "Visa", false), (305, "synthetic-card-number", true)]),
                ("Synthetic linked identity", [
                    (400, "Dr", false), (401, "Public", false), (402, "Synthetic Street One", false),
                    (403, "Synthetic Street Two", false), (404, "Synthetic Street Three", false),
                    (405, "Synthetic City", false), (406, "Synthetic State", false), (407, "00123", false),
                    (408, "Synthetic Country", false), (409, "Synthetic Company", false),
                    (410, "public@example.invalid", false), (411, "synthetic-phone", false),
                    (412, "synthetic-ssn", true), (413, "synthetic-identity-user", false),
                    (414, "synthetic-passport-number", true), (415, "synthetic-license-number", true),
                    (416, "Sample", false), (417, "Person", false), (418, "Dr Sample Public Person", false)])
            })
            {
                var item = overview.Items.Single(item => item.Name == name);
                var detail = Open(item.Id);
                foreach (var (id, value, sensitive) in expected)
                {
                    var field = detail.Fields.Single(field => field.Label == $"linked-{id}");
                    check(field.Value == value && field.Sensitive == sensitive, $"actual DLL resolves linked target {id} with correct sensitivity");
                }
                check(!RustCore.JsonForChecks(detail).Contains("stale-linked-storage-value", StringComparison.Ordinal),
                    $"actual DLL ignores stale linked literal values for {item.ItemType}");
                var editor = RustCore.GetVaultEditor(item.Id, "", CancellationToken.None);
                check(editor.CustomFields.Length == expected.Length && editor.CustomFields.All(field => field.Type == 3) &&
                    editor.CustomFields.Select(field => field.LinkedId).SequenceEqual(expected.Select(field => (int?)field.Id)),
                    $"detail reading preserves native linked IDs and field types for {item.ItemType}");
            }
            foreach (var (name, partial) in new[] { ("Synthetic hidden links", false), ("Synthetic partial links", true) })
            {
                var detail = Open(overview.Items.Single(item => item.Name == name).Id);
                check(detail.PasswordHidden == !partial && detail.Restricted == partial,
                    "server access flags reach the actual DLL for " + name);
                check((partial
                        ? detail.Fields.All(field => !field.Label.StartsWith("linked-", StringComparison.Ordinal))
                        : detail.Fields.Any(field => field.Label == "linked-100" && field.Value == "fixture-user")) &&
                    detail.Fields.All(field => !field.Sensitive && field.Label != "linked-101") &&
                    !RustCore.JsonForChecks(detail).Contains("synthetic-password-DO-NOT-USE", StringComparison.Ordinal),
                    "linked password remains absent under " + name);
            }
            var protectedId = overview.Items.Single(item => item.Name == "Synthetic protected item").Id;
            Expect(() => Open(protectedId), "passwordRequired", "linked detail requires the existing master-password reprompt");
            Expect(() => RustCore.GetVaultDetail(protectedId, "wrong", CancellationToken.None), "wrongPassword",
                "linked detail rejects an incorrect master password");
            check(RustCore.GetVaultDetail(protectedId, Password, CancellationToken.None).Fields.Any(field => field.Label == "linked-101" && field.Sensitive),
                "linked password appears only after actual SDK master-password verification");
            check(await Syncs() == syncs, "all linked detail and editor reads add zero server sync requests");

            var editableId = overview.Items.Single(item => item.Name == "Synthetic login").Id;
            var draft = RustCore.GetVaultEditor(editableId, "", CancellationToken.None);
            var values = new Dictionary<string, string>(draft.Values) { ["username"] = "updated-linked-user" };
            RustCore.UpdateVaultItem(draft with { Values = values }, "", CancellationToken.None);
            check(Open(editableId).Fields.Single(field => field.Label == "linked-100").Value == "updated-linked-user",
                "confirmed source edit refreshes the resolved linked value");
            check(RustCore.GetVaultEditor(editableId, "", CancellationToken.None).CustomFields.All(field => field.Type == 3 && field.LinkedId is 100 or 101),
                "confirmed source edit does not flatten associations into literal fields");
            var syncsBeforeLock = await Syncs();
            check(RustCore.Lock(), "linked fixture locks normally");
            Expect(() => Open(editableId), "locked", "locked native session refuses linked detail reads");
            RustCore.Unlock(Password, CancellationToken.None);
            check(Open(editableId).Fields.Single(field => field.Label == "linked-100").Value == "updated-linked-user" && await Syncs() == syncsBeforeLock,
                "offline in-process unlock restores links without another server sync");
            using (var cancelled = new CancellationTokenSource())
            {
                cancelled.Cancel();
                try { RustCore.GetVaultDetail(editableId, "", cancelled.Token); check(false, "cancelled linked detail unexpectedly completed"); }
                catch (OperationCanceledException) { check(true, "cancelled request cannot return a linked detail result"); }
            }
            RustCore.Logout();
            Expect(() => Open(editableId), "notAuthenticated", "logged-out session refuses linked detail reads");
        }
        finally
        {
            RustCore.Shutdown();
            // The cache writer lock intentionally lives until process exit. The host owns
            // this directory and deletes it after this test process has finished.
            await Control("reset");
        }

        VaultDetail Open(string id) => RustCore.GetVaultDetail(id, "", CancellationToken.None);
        void Expect(Action action, string code, string name)
        {
            try { action(); check(false, name); }
            catch (VaultException error) { check(error.Code == code, name); }
        }
        async Task Control(string operation)
        {
            using var response = await http.PostAsync("/control/" + operation, null);
            response.EnsureSuccessStatusCode();
        }
        async Task<int> Syncs() => (await http.GetFromJsonAsync("/control/snapshot", FixtureJsonContext.Default.JsonElement))
            .GetProperty("syncRequests").GetInt32();
    }
}
