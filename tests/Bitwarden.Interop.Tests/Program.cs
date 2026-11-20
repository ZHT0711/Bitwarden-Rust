using Bitwarden_WinUI.Interop;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;

var count = 0;
void Check(bool result, string name)
{
    if (!result) throw new Exception($"FAILED: {name}");
    count++;
    Console.WriteLine($"PASS: {name}");
}

var items = Enumerable.Range(1, 8).Select(type => new VaultItem(type.ToString(), $"Type {type}", "user@example.invalid", type,
    type == 1, type == 2 ? "folder" : null, null, false, false, false, false)).ToArray();
var mixed = items.Concat([items[0] with { Id = "deleted", Deleted = true }, items[1] with { Id = "archived", Archived = true }]).ToArray();
Check(items[0].ToString() == "Type 1", "accessible item names contain a label rather than a record dump");
Check(VaultQuery.Apply(mixed, "vault", "*", "").Length == 8, "normal vault excludes deleted and archived items");
Check(VaultQuery.Apply(mixed, "trash", "*", "").Single().Id == "deleted", "trash contains only deleted items");
Check(VaultQuery.Apply(mixed, "archived", "*", "").Single().Id == "archived", "archive excludes active items");
Check(VaultQuery.Apply(mixed, "favorites", "*", "").Single().Id == "1", "favorites exclude deleted favorite copies");
var categories = new[] { "logins", "notes", "cards", "identities", "ssh", "bankaccounts", "licenses", "passports" };
for (var index = 0; index < categories.Length; index++)
    Check(VaultQuery.Apply(mixed, categories[index], "*", "").Single().ItemType == index + 1, $"type filter {categories[index]}");
Check(VaultQuery.Apply(mixed, "vault", "folder", "").Single().Id == "2", "folder filter");
Check(VaultQuery.Apply(mixed, "vault", "", "").Length == 7, "no-folder filter");
Check(VaultQuery.Apply(mixed, "vault", "*", " TYPE 3 ").Single().Id == "3", "search trims whitespace and ignores case");
Check(VaultQuery.Apply(mixed, "vault", "*", "EXAMPLE.INVALID").Length == 8, "search includes displayed usernames");
Check(VaultQuery.Apply(mixed, "vault", "*", "no match").Length == 0, "unmatched search is empty");

if (args.Length > 0)
{
    var libraryPath = Path.GetFullPath(args[0]);
    NativeLibrary.SetDllImportResolver(typeof(RustCore).Assembly, (name, _, _) =>
        name == "bitwarden_core.dll" ? NativeLibrary.Load(libraryPath) : IntPtr.Zero);
    Check(RustCore.Version == "0.2.0", "managed interop loads current native core");
    Check(RustCore.GeneratePassword(24, true, true, true, true, false, 2, 2).Length == 24, "password generator ABI");
    Check(RustCore.IsValidServer(2, "http://127.0.0.1:23456"), "loopback server validation ABI");
    Check(!RustCore.IsValidServer(2, "http://example.invalid"), "insecure remote server is rejected");
    try { RustCore.GetVaultOverview(); throw new Exception("Unauthenticated overview was allowed."); }
    catch (VaultException error) { Check(error.Code == "notAuthenticated", "vault ABI denies unauthenticated access"); }
    var listener = new TcpListener(IPAddress.Loopback, 0);
    listener.Start();
    using var cancellation = new CancellationTokenSource();
    var port = ((IPEndPoint)listener.LocalEndpoint).Port;
    var login = Task.Run(() => RustCore.Login("fixture@example.invalid", "synthetic-password", 0, "", "", 2, $"http://127.0.0.1:{port}", Guid.NewGuid().ToString("D"), cancellation.Token));
    try
    {
        using var connection = await listener.AcceptTcpClientAsync().WaitAsync(TimeSpan.FromSeconds(10));
        cancellation.Cancel();
        var result = await login.WaitAsync(TimeSpan.FromSeconds(5));
        Check(result.Status == AuthStatus.Cancelled, "cancellation interrupts a hanging server through the actual ABI");
        Check(!RustCore.IsAuthenticated, "cancelled login leaves no authenticated session");
    }
    finally { cancellation.Cancel(); listener.Stop(); RustCore.Logout(); }
    if (args.Length > 1) await NativeLifecycle.RunAsync(args[1], Check);
}
Console.WriteLine($"{count} checks passed.");
