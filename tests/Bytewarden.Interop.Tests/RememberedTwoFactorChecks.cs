using Bytewarden.WinUI.Interop;
using System.Diagnostics;
using System.Net.Http.Json;
using System.Reflection;
using System.Text;

internal static class RememberedTwoFactorChecks
{
    private const string Email = "test@bitwarden.com";
    private const string Password = "asdfasdfasdf"; // Public synthetic SDK fixture only.
    private const string Device = "d8210c4d-2bd3-488b-8c63-f48628de94da";

    internal static async Task RunAsync(string dll, string address, Action<bool, string> check)
    {
        if (!OperatingSystem.IsWindows()) return;
        var uri = FixtureUri(address);
        using var http = new HttpClient { BaseAddress = uri };
        var health = await http.GetFromJsonAsync("/health", FixtureJsonContext.Default.JsonElement);
        if (!health.GetProperty("fixture").GetBoolean()) throw new Exception("Not a synthetic service.");
        var peer = health.TryGetProperty("peerPort", out var port) ? new UriBuilder(uri) { Port = port.GetInt32() }.Uri : uri;
        using var other = new HttpClient { BaseAddress = peer };
        var root = Directory.CreateTempSubdirectory("bytewarden-remember-check-");
        try
        {
            await Control(http, "reset");
            await Control(other, "reset");
            await Control(http, "require-authenticator");
            await Control(other, "require-authenticator");
            await RunChild("seed");
            var directory = Path.Combine(root.FullName, "vault-cache");
            var file = Directory.GetFiles(directory, "*.two-factor").Single();
            var original = File.ReadAllBytes(file);
            check(new[] { Email, Password, "synthetic-remember-", "issued_at", "token" }
                .All(value => original.AsSpan().IndexOf(Encoding.UTF8.GetBytes(value)) < 0),
                "remembered 2FA credential and identity are DPAPI-protected on disk");
            check(Directory.GetFiles(directory, "*.bin").Length == 0, "remembered 2FA persists even before first vault sync without restoring an account");
            await RunChild("reuse");
            check(File.ReadAllBytes(file).SequenceEqual(original), "restart/logout reuse never slides the thirty-day expiry");
            await Control(http, "revoke-remembered-two-factor");
            await RunChild("challenge");
            check(!File.Exists(file), "server revocation clears the remembered credential");
            await RunChild("seed");
            await Control(http, "revoke-remembered-two-factor");
            await Control(http, "remember-invalid-code");
            await RunChild("challenge");
            check(!File.Exists(file), "legacy invalid-code revocation falls back to a real 2FA challenge and removes the token");
            await RunChild("seed");
            var corrupted = File.ReadAllBytes(file);
            corrupted[0] ^= 0x40;
            File.WriteAllBytes(file, corrupted);
            await RunChild("challenge");
            check(!File.Exists(file), "tampered persisted credentials fail closed and are discarded");
            await RunChild("unchecked");
            check(!File.Exists(file), "unchecked login never persists a remembered credential");
            await Control(http, "omit-next-remember-token");
            await RunChild("missing");
            check(!File.Exists(file), "client never fabricates a grant when the service omits its token");
            await Control(http, "pause-next-token");
            await RunChild("cancelled");
            check(!File.Exists(file), "cancelled checked login cannot persist a server-issued token");
        }
        finally
        {
            root.Delete(recursive: true);
            await Control(http, "reset");
            await Control(other, "reset");
        }

        async Task RunChild(string step)
        {
            var info = new ProcessStartInfo(Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") ?? "dotnet") { UseShellExecute = false };
            foreach (var arg in new[] { Assembly.GetExecutingAssembly().Location, "--remember-child", dll, root.FullName,
                uri.GetLeftPart(UriPartial.Authority), peer.GetLeftPart(UriPartial.Authority), step }) info.ArgumentList.Add(arg);
            using var child = Process.Start(info) ?? throw new Exception("Cannot launch remembered 2FA check.");
            try
            {
                await child.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(40));
                check(child.ExitCode == 0, $"actual DLL remembered-2FA subprocess: {step}");
            }
            finally
            {
                if (!child.HasExited) { child.Kill(entireProcessTree: true); await child.WaitForExitAsync(); }
            }
        }
    }

    internal static async Task ChildAsync(string[] args)
    {
        var (root, address, peer, step) = (args[2], args[3], args[4], args[5]);
        Require(RustCore.ConfigureLocalCache(root), "Windows credential storage configured");
        Require(RustCore.GetAccounts().Length == 0 && !RustCore.IsAuthenticated, "remembering 2FA does not authenticate or unlock at startup");
        AuthResult Login(string code = "", bool remember = false, string? device = null, string? server = null, string? password = null) =>
            RustCore.Login(Email, password ?? Password, 0, code, "", 2, server ?? address, device ?? Device, rememberTwoFactor: remember);
        switch (step)
        {
            case "seed":
                Require(Login().Status == AuthStatus.TwoFactorRequired, "fresh login requires 2FA");
                Require(Login("123456", true).Status == AuthStatus.InvalidTwoFactorCode, "checked invalid code cannot grant trust");
                Require(Directory.GetFiles(Path.Combine(root, "vault-cache"), "*.two-factor").Length == 0, "no token saved after invalid code");
                Require(Login("087349", true).Status == AuthStatus.Authenticated, "checked successful login is accepted");
                RustCore.Shutdown();
                break;
            case "reuse":
                Require(Login(device: Guid.NewGuid().ToString("D")).Status == AuthStatus.TwoFactorRequired, "different device still requires 2FA");
                if (peer != address) Require(Login(server: peer).Status == AuthStatus.TwoFactorRequired, "same email on different server still requires 2FA");
                var rejectedPassword = Login(password: "wrong synthetic password");
                Require(rejectedPassword.Status == AuthStatus.AuthenticationRejected,
                    $"wrong-password authentication retains its typed rejection ({rejectedPassword.Status})");
                Require(!RustCore.IsAuthenticated, "remembered token never replaces a valid master password");
                Require(Login(device: "{" + Device.ToUpperInvariant() + "}").Status == AuthStatus.Authenticated,
                    "equivalent UUID spellings reuse the same canonical device-bound credential");
                RustCore.Logout();
                Require(Login().Status == AuthStatus.Authenticated, "restart reuses server-issued credential without an OTP");
                RustCore.Lock();
                Require(RustCore.SessionState == VaultSessionState.Locked, "remembered 2FA does not bypass vault locking");
                RustCore.Logout();
                Require(Login().Status == AuthStatus.Authenticated, "explicit logout preserves user's remembered-2FA grant like upstream");
                RustCore.Shutdown();
                break;
            case "challenge":
                var challenge = Login();
                Require(challenge.Status == AuthStatus.TwoFactorRequired && challenge.AvailableTwoFactorMethods == 1 && !RustCore.IsAuthenticated,
                    "unusable token restores advertised authenticator challenge, not authentication");
                Require(Login("087349").Status == AuthStatus.Authenticated, "ordinary 2FA still works after fallback");
                RustCore.Logout();
                break;
            case "unchecked":
            case "missing":
                Require(Login().Status == AuthStatus.TwoFactorRequired, "ordinary login requires a code");
                Require(Login("087349", step == "missing").Status == AuthStatus.Authenticated, "nonremembered successful 2FA works");
                RustCore.Logout();
                Require(Login().Status == AuthStatus.TwoFactorRequired, "next login still requires 2FA when no grant was saved");
                break;
            case "cancelled":
                using (var cancellation = new CancellationTokenSource(TimeSpan.FromSeconds(1)))
                {
                    var result = await Task.Run(() => RustCore.Login(Email, Password, 0, "087349", "", 2, address, Device, cancellation.Token, true));
                    Require(result.Status == AuthStatus.Cancelled && !RustCore.IsAuthenticated, "cancelling checked login leaves the client signed out");
                }
                break;
            default: throw new ArgumentException("Unknown remembered-2FA test step.");
        }
    }

    private static Uri FixtureUri(string address)
    {
        if (!Uri.TryCreate(address, UriKind.Absolute, out var uri) || !uri.IsLoopback || uri.Scheme != "http")
            throw new ArgumentException("Remembered-2FA tests only allow synthetic loopback HTTP.");
        return uri;
    }
    private static async Task Control(HttpClient http, string command)
    {
        using var result = await http.PostAsync("/control/" + command, null);
        result.EnsureSuccessStatusCode();
    }
    private static void Require(bool condition, string message)
    {
        if (!condition) throw new Exception("FAILED: " + message);
        Console.WriteLine("PASS child: " + message);
    }
}
