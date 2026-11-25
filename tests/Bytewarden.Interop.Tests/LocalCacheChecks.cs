using Bytewarden.WinUI.Interop;
using System.Diagnostics;
using System.Net.Http.Json;
using System.Reflection;
using System.Text;
using System.Text.Json;

internal static class LocalCacheChecks
{
    private const string Email = "test@bitwarden.com";
    private const string Password = "asdfasdfasdf"; // Public pinned SDK fixture only.
    private const string Device = "8128f377-891d-4c7d-821a-c28a063537aa";
    private const string PrimaryNote = "Primary cache fixture note";
    private const string SecondaryNote = "Secondary cache fixture note";
    private const string InterruptedNote = "Committed while the client was terminated";

    internal static async Task RunAsync(string dll, string address, Action<bool, string> check)
    {
        if (!OperatingSystem.IsWindows()) return;
        var primary = FixtureUri(address);
        using var http = new HttpClient { BaseAddress = primary, Timeout = TimeSpan.FromSeconds(5) };
        var health = await Health(http);
        if (!health.TryGetProperty("peerPort", out var port)) return;
        var peer = new UriBuilder(primary) { Port = port.GetInt32() }.Uri;
        using var peerHttp = new HttpClient { BaseAddress = peer, Timeout = http.Timeout };
        await Health(peerHttp);
        var root = Directory.CreateTempSubdirectory("bytewarden-cache-check-");
        try
        {
            await Control(http, "reset");
            await Control(peerHttp, "reset");
            await RunChild("seed");
            var cache = Path.Combine(root.FullName, "vault-cache");
            var files = Directory.GetFiles(cache, "*.bin");
            check(files.Length == 2, "each server has its own persisted vault file");
            check(files.Select(File.ReadAllBytes).All(bytes => new[] { PrimaryNote, SecondaryNote, Email, Password }
                .All(value => !bytes.AsSpan().ContainsBytes(Encoding.UTF8.GetBytes(value)))),
                "the Windows cache exposes neither fixture identity nor vault names or password on disk");

            await RunChild("offline");
            await RunChild("reconnect");
            await RunChild("journal");
            var before = await CipherCount(http);
            await Control(http, "pause-next-write-response");
            using (var interrupted = Launch("interrupted"))
            {
                try
                {
                    using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
                    while (await CipherCount(http) == before && !interrupted.HasExited)
                        await Task.Delay(30, timeout.Token);
                    check(!interrupted.HasExited && Directory.GetFiles(cache, "*.pending").Length == 1,
                        "the write-ahead marker exists while the committed write is awaiting acknowledgment");
                }
                finally
                {
                    if (!interrupted.HasExited) interrupted.Kill(entireProcessTree: true);
                    await interrupted.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(10));
                }
            }
            await RunChild("reconcile");
            check(Directory.GetFiles(cache, "*.pending").Length == 0 && Directory.GetFiles(cache, "*.bin").Length == 1,
                "reconciliation clears the journal and logout removes only its account's cache");
            await RunChild("rotate");
            await RunChild("empty");
            check(Directory.GetFiles(cache, "*.bin").Length == 0, "key rotation clears stale material even when the sync is rejected");

            await Control(http, "reset");
            await Control(peerHttp, "reset");
            await RunChild("seed");
            await RunChild("logout-busy-file");
            check(Directory.GetFiles(cache, "*.bin").Length == 1 && Directory.GetFiles(cache, "*.revoked").Length == 1,
                "a cache whose deletion was denied is marked as revoked instead of trusted at startup");
            await RunChild("empty");
        }
        finally
        {
            root.Delete(recursive: true);
            await Control(http, "reset");
            await Control(peerHttp, "reset");
        }

        Process Launch(string step)
        {
            var info = new ProcessStartInfo(Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") ?? "dotnet")
            {
                UseShellExecute = false,
            };
            foreach (var arg in new[] { Assembly.GetExecutingAssembly().Location, "--cache-child", dll, root.FullName,
                primary.GetLeftPart(UriPartial.Authority), peer.GetLeftPart(UriPartial.Authority), step })
                info.ArgumentList.Add(arg);
            return Process.Start(info) ?? throw new Exception("Unable to launch the cache test child.");
        }
        async Task RunChild(string step)
        {
            using var child = Launch(step);
            try
            {
                await child.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(40));
                check(child.ExitCode == 0, $"actual native cache subprocess completed: {step}");
            }
            finally
            {
                if (!child.HasExited)
                {
                    child.Kill(entireProcessTree: true);
                    await child.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(10));
                }
            }
        }
    }

    internal static async Task ChildAsync(string[] args)
    {
        var (root, primary, peer, step) = (args[2], args[3], args[4], args[5]);
        using var http = new HttpClient { BaseAddress = FixtureUri(primary), Timeout = TimeSpan.FromSeconds(5) };
        using var other = new HttpClient { BaseAddress = FixtureUri(peer), Timeout = http.Timeout };
        await Health(http);
        await Health(other);
        Require(RustCore.ConfigureLocalCache(root), "native Windows cache is configured");
        var accounts = RustCore.GetAccounts();
        var token = CancellationToken.None;
        switch (step)
        {
            case "seed":
                Require(accounts.Length == 0, "starts with an empty disk cache");
                Login(primary);
                var fresh = RustCore.GetAccounts().Single(account => account.Active);
                Expect("notSynced", () => RustCore.EnableHello(fresh.Id, Password, token));
                RustCore.SyncVault(token);
                Expect("wrongPassword", () => RustCore.EnableHello(fresh.Id, "wrong fixture password", token));
                Require(!RustCore.GetAccounts().Single(account => account.Active).HelloEnabled, "Hello enrollment is opt-in and requires master password confirmation");
                CreateNote(PrimaryNote);
                Login(peer);
                RustCore.SyncVault(token);
                CreateNote(SecondaryNote);
                RustCore.Shutdown();
                Require(RustCore.SessionState == VaultSessionState.SignedOut, "graceful exit clears in-memory sessions");
                break;
            case "offline":
                Require(accounts.Length == 2 && accounts.All(account => account is { State: VaultSessionState.Locked, NeedsSignIn: true }),
                    "both previously synced accounts restart locked and without server tokens");
                var a = Select(primary);
                Require(!a.HelloEnabled, "restarting does not implicitly enable Windows Hello");
                Expect("helloNotEnabled", () => RustCore.UnlockHello(a.Id, token));
                Expect("unlockUnavailable", () => RustCore.UnlockHello(accounts.Single(account => account.Id != a.Id).Id, token));
                Require(RustCore.SessionState == VaultSessionState.Locked, "rejecting unenrolled or inactive Hello leaves the vault locked");
                Expect("locked", () => RustCore.GetVaultOverview());
                Expect("wrongPassword", () => RustCore.Unlock("wrong fixture password", token));
                Require(RustCore.SessionState == VaultSessionState.Locked, "a wrong password never unlocks the cached vault");
                var syncs = await SyncCount(http);
                var logins = await PasswordCount(http);
                RustCore.Unlock(Password, token);
                Require(ReadOnlySnapshotContains(PrimaryNote, SecondaryNote), "primary restart restores the latest local snapshot read-only");
                var view = RustCore.GetVaultOverview();
                var protectedItem = view.Items.Single(item => item.Name == "Synthetic protected item");
                Expect("passwordRequired", () => RustCore.GetVaultDetail(protectedItem.Id, "", token));
                Expect("wrongPassword", () => RustCore.GetVaultDetail(protectedItem.Id, "wrong fixture password", token));
                var approved = RustCore.GetVaultDetail(protectedItem.Id, Password, token);
                Require(approved.AttachmentFiles is [{ Name: "synthetic-attachment.txt" }] && !approved.CanUploadAttachments &&
                    !approved.CanDeleteAttachments && approved.AttachmentRequestId is null, "local reprompt decrypts attachment metadata, not network permissions");
                var item = view.Items.Single(item => item.Name == "Synthetic login");
                Require(RustCore.GetVaultDetail(item.Id, "", token).Totp is { Status: "ready" }, "local cache supports TOTP generation");
                Expect("notSynced", () => CreateNote("This must not be sent"));
                Expect("signInRequired", () => RustCore.SyncVault(token));
                Require(await SyncCount(http) == syncs && await PasswordCount(http) == logins,
                    "offline unlock, reprompt and sync refusal make no login or API sync request");
                Select(peer);
                RustCore.Unlock(Password, token);
                Require(ReadOnlySnapshotContains(SecondaryNote, PrimaryNote), "same email on another server unlocks a different cache");
                Require(RustCore.GetAccounts().Single(account => account.ServerUrl == primary).Id == a.Id,
                    "account switch retains the first identity");
                RustCore.Shutdown();
                break;
            case "reconnect":
                var restored = Select(primary);
                var primaryPasswords = await PasswordCount(http);
                var primarySyncs = await SyncCount(http);
                var peerPasswords = await PasswordCount(other);
                Require(restored is { State: VaultSessionState.Locked, NeedsSignIn: true }, "reconnect starts directly from the locked restart account");
                Login(primary); // No preceding local unlock.
                Require(RustCore.GetAccounts().Single(account => account.Active) is { State: VaultSessionState.Unlocked, NeedsSignIn: false } current && current.Id == restored.Id,
                    "one password login unlocks and reconnects the same account");
                await Control(http, "fail-next-sync");
                Expect("network", () => RustCore.SyncVault(token));
                Require(ReadOnlySnapshotContains(PrimaryNote, SecondaryNote), "failed first sync keeps the old snapshot read-only");
                Expect("notSynced", () => CreateNote("This must not be sent before reconnect sync"));
                Require(!RustCore.SyncVault(token).ReadOnly, "retrying sync restores write access without asking for the password again");
                Require(await PasswordCount(http) == primaryPasswords + 1 && await SyncCount(http) == primarySyncs + 2 && await PasswordCount(other) == peerPasswords,
                    "reconnect and sync only contacted the selected server and used one password grant");

                var restoredPeer = Select(peer);
                await Control(other, "require-authenticator");
                Require(RustCore.Login(Email, Password, 0, "", "", 2, peer, Device).Status == AuthStatus.TwoFactorRequired,
                    "a locked restart account still requires the self-hosted server's second factor");
                Require(RustCore.SessionState == VaultSessionState.Locked && !RustCore.IsAuthenticated, "a challenge does not unlock the local vault");
                await Control(other, "pause-next-token");
                using (var cancel = new CancellationTokenSource())
                {
                    var pending = Task.Run(() => RustCore.Login(Email, Password, 0, "087349", "", 2, peer, Device, cancel.Token));
                    using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
                    while (await PasswordCount(other) < peerPasswords + 2 && !pending.IsCompleted)
                        await Task.Delay(20, deadline.Token);
                    Require(!pending.IsCompleted, "cancellation interrupts a real reconnect request");
                    cancel.Cancel();
                    Require((await pending.WaitAsync(TimeSpan.FromSeconds(5))).Status == AuthStatus.Cancelled && RustCore.SessionState == VaultSessionState.Locked,
                        "cancelled reconnect leaves the cached vault locked");
                }
                Require(RustCore.Login(Email, Password, 0, "087349", "", 2, peer, Device).Status == AuthStatus.Authenticated &&
                    RustCore.GetAccounts().Single(account => account.Active).Id == restoredPeer.Id, "valid second factor unlocks and reconnects without a separate unlock step");
                Require(!RustCore.SyncVault(token).ReadOnly, "2FA reconnect restores online vault access after sync");
                await Control(other, "reset");
                RustCore.Shutdown();
                break;
            case "journal":
                Select(primary);
                Login(primary);
                RustCore.SyncVault(token);
                var directory = Path.Combine(root, "vault-cache");
                var cache = CacheFile(primary);
                var snapshot = File.ReadAllBytes(cache);
                Expect("validation", () => CreateNote(""));
                Require(Directory.GetFiles(directory, "*.pending").Length == 0 && File.ReadAllBytes(cache).SequenceEqual(snapshot),
                    "a locally rejected write removes its own marker without rewriting the snapshot");
                using (File.Open(cache, FileMode.Open, FileAccess.Read, FileShare.Read))
                {
                    CreateNote("Acknowledged write with a busy cache file");
                }
                Expect("validation", () => CreateNote(""));
                Require(Directory.GetFiles(directory, "*.pending").Length == 1 && File.ReadAllBytes(cache).SequenceEqual(snapshot),
                    "a locally rejected write does not clear an older marker left by failed snapshot persistence");
                RustCore.SyncVault(token);
                Require(Directory.GetFiles(directory, "*.pending").Length == 0, "successful sync clears the older marker");
                RustCore.Shutdown();
                break;
            case "interrupted":
                Select(primary);
                Login(primary);
                RustCore.SyncVault(token);
                CreateNote(InterruptedNote);
                throw new Exception("Test client was not terminated during the pending write.");
            case "reconcile":
                var id = Select(primary).Id;
                Expect("locked", () => RustCore.GetVaultOverview());
                Login(primary);
                Require(RustCore.GetAccounts().Single(account => account.Active).Id == id, "server reauthentication keeps the local account id");
                Expect("notSynced", () => RustCore.GetVaultOverview());
                Expect("notSynced", () => CreateNote("Do not duplicate an interrupted write"));
                var server = RustCore.SyncVault(token);
                Require(!server.ReadOnly && server.Items.Count(item => item.Name == InterruptedNote) == 1,
                    "a fresh sync reconciles the committed write and restores write access");
                RustCore.Logout();
                Require(RustCore.GetAccounts().Single().ServerUrl == peer, "explicit logout keeps only the other cached account");
                RustCore.Shutdown();
                break;
            case "rotate":
                Require(accounts.Length == 1 && accounts[0].ServerUrl == peer, "signed-out account does not return after restart");
                Login(peer);
                await Control(other, "change-master-password-wrapper");
                await Control(other, "duplicate-next-sync-cipher");
                Expect("invalidResponse", () => RustCore.SyncVault(token));
                Require(!File.Exists(CacheFile(peer)), "rotation revokes the old snapshot before retrying a rejected sync");
                RustCore.Shutdown();
                break;
            case "logout-busy-file":
                Select(primary);
                using (File.Open(CacheFile(primary), FileMode.Open, FileAccess.Read, FileShare.Read))
                {
                    RustCore.Logout();
                    Require(RustCore.GetAccounts().Single().ServerUrl == peer, "logout only removes the active account");
                }
                RustCore.LogoutAll();
                Require(RustCore.GetAccounts().Length == 0, "logout-all removes the remaining account");
                break;
            case "empty":
                Require(accounts.Length == 0, "a revoked cache cannot be unlocked after another restart");
                break;
            default: throw new ArgumentException("Unknown cache test scenario.");
        }

        string CacheFile(string server)
        {
            var identity = $"Bytewarden/WindowsVaultCache/v1/{server}/api\n{Email}";
            var digest = System.Security.Cryptography.SHA256.HashData(Encoding.UTF8.GetBytes(identity));
            return Path.Combine(root, "vault-cache", Convert.ToHexStringLower(digest) + ".bin");
        }
        VaultAccount Select(string server)
        {
            var account = accounts.Single(account => account.ServerUrl == server);
            RustCore.SwitchAccount(account.Id);
            return account;
        }
        bool ReadOnlySnapshotContains(string included, string excluded)
        {
            var snapshot = RustCore.GetVaultOverview();
            return snapshot.ReadOnly && snapshot.Items.Any(item => item.Name == included) && snapshot.Items.All(item => item.Name != excluded);
        }
        void Login(string server) => Require(RustCore.Login(Email, Password, 0, "", "", 2, server, Device).Status == AuthStatus.Authenticated,
            "synthetic login succeeds");
        void CreateNote(string name) => RustCore.CreateVaultItem(new VaultEditorData(null, name, 2, null,
            false, false, null, "Synthetic test content", [], [], [], true, false, false, false, false), token);
    }

    private static bool ContainsBytes(this Span<byte> bytes, byte[] value) => bytes.IndexOf(value) >= 0;
    private static Uri FixtureUri(string address) => Uri.TryCreate(address, UriKind.Absolute, out var uri) && uri.IsLoopback && uri.Scheme == "http"
        ? uri : throw new ArgumentException("Only a loopback fixture may be used.");
    private static async Task<JsonElement> Health(HttpClient http)
    {
        var health = await http.GetFromJsonAsync("/health", FixtureJsonContext.Default.JsonElement);
        Require(health.GetProperty("fixture").GetBoolean(), "server identifies itself as a synthetic fixture");
        return health;
    }
    private static async Task Control(HttpClient http, string command)
    {
        using var response = await http.PostAsync($"/control/{command}", null);
        response.EnsureSuccessStatusCode();
    }
    private static async Task<int> CipherCount(HttpClient http) => (await http.GetFromJsonAsync("/control/snapshot", FixtureJsonContext.Default.JsonElement)).GetProperty("cipherCount").GetInt32();
    private static async Task<int> SyncCount(HttpClient http) => (await http.GetFromJsonAsync("/control/snapshot", FixtureJsonContext.Default.JsonElement)).GetProperty("syncRequests").GetInt32();
    private static async Task<int> PasswordCount(HttpClient http) => (await http.GetFromJsonAsync("/control/snapshot", FixtureJsonContext.Default.JsonElement)).GetProperty("passwordRequests").GetInt32();
    private static void Expect(string code, Action action)
    {
        try { action(); }
        catch (VaultException error) when (error.Code == code) { Console.WriteLine($"PASS: cache guard returns {code}"); return; }
        throw new Exception($"Cache guard failed to return {code}.");
    }
    private static void Require(bool condition, string message)
    {
        if (!condition) throw new Exception($"FAILED: {message}");
        Console.WriteLine($"PASS: {message}");
    }
}
