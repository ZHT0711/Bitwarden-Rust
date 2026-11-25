using Bytewarden.WinUI.Interop;
using System.Diagnostics;
using System.Net;
using System.Security.Cryptography;
using System.Text.Json;

internal static class SendChecks
{
    internal static async Task RunAsync(Uri fixture, Action<bool, string> check)
    {
        using var http = new HttpClient { BaseAddress = fixture, Timeout = TimeSpan.FromSeconds(5) };
        var token = CancellationToken.None;
        var account = RustCore.GetAccounts().Single(current => current.Active);
        check(RustCore.ListSends(account.Id, token).Items.Length == 0, "Send starts empty on the loopback fixture");
        Throws<OperationCanceledException>(() => RustCore.ListSends(Guid.NewGuid().ToString(), token));
        check(true, "Send rejects a request for a non-active account");
        var draft = new SendDraft("Synthetic text Send", "合成 Send 正文", "Private synthetic note", true, false, true, 3,
            DateTimeOffset.UtcNow.AddDays(7).ToString("O"), DateTimeOffset.UtcNow.AddDays(5).ToString("O"), "test fixture password", null);
        Expect("validation", () => RustCore.SaveSend(account.Id, null, draft with { Text = string.Empty }, token));
        Expect("validation", () => RustCore.SaveSend(account.Id, null, draft with { Emails = ["one@example.invalid"] }, token));
        RustCore.SaveSend(account.Id, null, draft, token);
        var listing = RustCore.ListSends(account.Id, token);
        var item = listing.Items.Single();
        check(listing.Failures == 0 && item.Name == draft.Name && item.Kind == 0 && item.StatusLabel == "有效", "Send list decrypts SDK metadata only");
        var detail = RustCore.GetSend(account.Id, item.Id, token);
        check(detail.Text == draft.Text && detail.Notes == draft.Notes && detail.Hidden && detail.HideEmail && detail.HasPassword && detail.MaxAccessCount == 3,
            "selected Send returns encrypted-and-round-tripped text, notes and access settings");
        var detailJson = JsonSerializer.Serialize(detail, FixtureJsonContext.Default.SendDetail);
        check(!detailJson.Contains(draft.Password!, StringComparison.Ordinal) && !detailJson.Contains("\"key\"", StringComparison.Ordinal),
            "Send detail never returns the sharing key or plaintext password");
        var firstLink = RustCore.GetSendLink(account.Id, item.Id, token);
        var parsedLink = new Uri(firstLink);
        var fragments = parsedLink.Fragment.TrimStart('#').Split('/', StringSplitOptions.RemoveEmptyEntries);
        check(parsedLink.GetLeftPart(UriPartial.Authority) == fixture.GetLeftPart(UriPartial.Authority) &&
            fragments is ["send", { Length: 22 }, { Length: 22 }], "Send links target the account server and encode both access ID and decryption key in the fragment");
        RustCore.SaveSend(account.Id, detail, draft with { Name = "Updated Send", Password = null, Disabled = true,
            Notes = null, ExpirationDate = null, MaxAccessCount = null }, token);
        var updated = RustCore.GetSend(account.Id, item.Id, token);
        check(updated.Name == "Updated Send" && updated.HasPassword && updated.Disabled && RustCore.GetSendLink(account.Id, item.Id, token) == firstLink,
            "editing preserves password protection and the share link");
        check(updated is { Notes: null, ExpirationDate: null, MaxAccessCount: null },
            "clearing optional fields works against a server that preserves omitted settings");
        Expect("conflict", () => RustCore.DeleteSend(account.Id, item, token));
        RustCore.RemoveSendAuth(account.Id, updated, token);
        check(RustCore.GetSend(account.Id, item.Id, token) is { AuthType: 2, HasPassword: false }, "explicit remove-auth clears the password using the legacy-compatible route");
        var current = RustCore.ListSends(account.Id, token).Items.Single();
        RustCore.DeleteSend(account.Id, current, token);
        Expect("notFound", () => RustCore.GetSendLink(account.Id, item.Id, token));
        using (var response = await http.PostAsync("/control/drop-next-send-response", new StringContent("{}", System.Text.Encoding.UTF8, "application/json")))
            response.EnsureSuccessStatusCode();
        Expect("writeUncertain", () => RustCore.SaveSend(account.Id, null, draft, token));
        Expect("writeUncertain", () => RustCore.SaveSend(account.Id, null, draft, token));
        var reconciled = RustCore.ListSends(account.Id, token).Items;
        check(reconciled.Length == 1 && reconciled[0].Name == draft.Name, "lost Send acknowledgment requires refresh and does not create a duplicate");
        RustCore.DeleteSend(account.Id, reconciled.Single(), token);
        await CheckFilesAsync();
        check(RustCore.Lock(), "Send checks can lock the vault");
        Expect("locked", () => RustCore.ListSends(account.Id, token));
        RustCore.Unlock("asdfasdfasdf", token); // Public SDK fixture only.

        async Task CheckFilesAsync()
        {
            var directory = Directory.CreateTempSubdirectory("bytewarden-file-send-");
            try
            {
                var path = Path.Combine(directory.FullName, "合成共享文件.bin");
                var bytes = new byte[130_019];
                new Random(589).NextBytes(bytes);
                await File.WriteAllBytesAsync(path, bytes);
                var file = draft with { Name = "Synthetic file Send", Text = null, Hidden = false };
                void Upload(CancellationToken cancellation = default) => RustCore.CreateFileSend(account.Id, file, path, cancellation);
                SendSummary[] Items() => RustCore.ListSends(account.Id, token).Items;
                SendDetail Details(string id) => RustCore.GetSend(account.Id, id, token);
                Expect("attachmentFileRead", () => RustCore.CreateFileSend(account.Id, file, "relative.bin", token));
                Expect("validation", () => RustCore.CreateFileSend(account.Id, draft, path, token));
                var oversized = Path.Combine(directory.FullName, "oversized.bin");
                using (var stream = File.Create(oversized)) stream.SetLength(500L * 1024 * 1024 + 1);
                Expect("attachmentTooLarge", () => RustCore.CreateFileSend(account.Id, file, oversized, token));
                check(await TransferCount("started") == 0 && Items().Length == 0,
                    "invalid File Send input does not reserve a server record or upload bytes");

                Upload();
                var direct = Items().Single();
                var directDetail = Details(direct.Id);
                check(direct.Kind == 1 && directDetail.FileName == Path.GetFileName(path) && directDetail.Text is null && directDetail.HasPassword,
                    "direct File Send decrypts the filename on demand and preserves the selected authentication");
                var link = RustCore.GetSendLink(account.Id, direct.Id, token);
                await CheckCiphertext(direct, link, bytes);
                Expect("validation", () => RustCore.SaveSend(account.Id, directDetail, draft, token));
                RustCore.SaveSend(account.Id, directDetail, file with { Name = "Renamed file Send", Password = null }, token);
                check(Details(direct.Id) is { Name: "Renamed file Send", HasPassword: true } edited &&
                    edited.FileName == directDetail.FileName && RustCore.GetSendLink(account.Id, direct.Id, token) == link,
                    "File Send metadata edit preserves the payload, password and recipient link");
                await CheckCiphertext(direct, link, bytes);
                RustCore.DeleteSend(account.Id, Items().Single(), token);
                using (var removed = await http.GetAsync($"/fixture-send/{direct.Id}"))
                    check(removed.StatusCode == HttpStatusCode.NotFound, "deleting a File Send also removes its download from the fixture");

                await Control("azure-next-send");
                Upload();
                var signed = Items().Single();
                check(await TransferCount("renewals") == 1, "File Send renews an expired signed slot before uploading without account headers");
                await CheckCiphertext(signed, RustCore.GetSendLink(account.Id, signed.Id, token), bytes);
                RustCore.DeleteSend(account.Id, signed, token);

                await Control("fail-next-send-upload");
                Expect("sendFileUploadFailed", () => Upload());
                check(Items().Length == 0, "failed File Send upload rolls back its server record");
                var started = await TransferCount("started");
                await Control("pause-next-send-upload");
                using (var cancellation = new CancellationTokenSource())
                {
                    var upload = Task.Run(() => Upload(cancellation.Token));
                    try
                    {
                        var timer = Stopwatch.StartNew();
                        while (await TransferCount("started") == started && timer.Elapsed < TimeSpan.FromSeconds(5))
                            await Task.Delay(25);
                        check(await TransferCount("started") == started + 1, "File Send cancellation interrupts a confirmed in-flight upload");
                    }
                    finally { cancellation.Cancel(); }
                    try { await upload; throw new Exception("Expected upload cancellation."); }
                    catch (OperationCanceledException) { }
                    check(Items().Length == 0, "cancelling File Send removes the reserved server record");
                }

                await Control("fail-next-send-upload");
                await Control("fail-next-send-delete");
                Expect("writeUncertain", () => Upload());
                Expect("writeUncertain", () => Upload());
                RustCore.DeleteSend(account.Id, Items().Single(), token);
                check(Items().Length == 0, "failed rollback blocks duplicate creates until listing and explicit cleanup");
            }
            finally { directory.Delete(recursive: true); }
        }

        async Task CheckCiphertext(SendSummary item, string link, byte[] original)
        {
            using var document = JsonDocument.Parse(await http.GetStringAsync($"/fixture-send/{item.Id}"));
            var payload = document.RootElement.GetProperty("ciphertext").GetBytesFromBase64();
            // Independently model the receiving client. Only the fragment contains its 16-byte key.
            var seed = Convert.FromBase64String(link.Split('/')[^1].Replace('-', '+').Replace('_', '/') + "==");
            var prk = HMACSHA256.HashData("bitwarden-send"u8, seed);
            var keys = HKDF.Expand(HashAlgorithmName.SHA256, prk, 64, "send"u8.ToArray());
            var iv = payload[1..17];
            var data = payload[49..];
            var mac = HMACSHA256.HashData(keys.AsSpan(32), [.. iv, .. data]);
            using var aes = Aes.Create();
            aes.Key = keys[..32];
            check(payload[0] == 2 && CryptographicOperations.FixedTimeEquals(mac, payload.AsSpan(17, 32)) &&
                aes.DecryptCbc(data, iv).AsSpan().SequenceEqual(original),
                "recipient fragment authenticates and decrypts the server's file bytes exactly");
        }

        async Task Control(string action)
        {
            using var response = await http.PostAsync($"/control/{action}", null);
            response.EnsureSuccessStatusCode();
        }

        async Task<int> TransferCount(string field)
        {
            using var response = JsonDocument.Parse(await http.GetStringAsync("/control/send-transfers"));
            return response.RootElement.GetProperty(field).GetInt32();
        }

        void Expect(string code, Action request)
        {
            try { request(); throw new Exception($"Expected Send error {code}."); }
            catch (VaultException error) { check(error.Code == code, $"Send returns {code} (received {error.Code})"); }
        }
    }

    private static void Throws<T>(Action action) where T : Exception
    {
        try { action(); }
        catch (T) { return; }
        throw new Exception($"Expected {typeof(T).Name}.");
    }
}
