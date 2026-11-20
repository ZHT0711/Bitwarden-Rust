using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

internal static class WebsiteIcons
{
    private const string PreferenceKey = "ShowWebsiteIcons";
    private static readonly HttpClient Client = new(new HttpClientHandler
    {
        AllowAutoRedirect = false,
        UseCookies = false,
        MaxConnectionsPerServer = 6,
    })
    {
        Timeout = TimeSpan.FromSeconds(6),
        MaxResponseContentBufferSize = 128 * 1024,
    };
    private static readonly Dictionary<Uri, Task<byte[]?>> Cache = new();
    private static CancellationTokenSource _requests = new();
    private static Uri? _endpoint;

    internal static bool Enabled { get; private set; } = ReadPreference();

    internal static void SetEnabled(bool enabled)
    {
        Enabled = enabled;
        if (!enabled) ClearCache();
        try { ApplicationData.Current.LocalSettings.Values[PreferenceKey] = enabled; }
        catch (Exception error) when (error is COMException or InvalidOperationException) { }
    }

    internal static void SetServer(uint mode, string address)
    {
        ClearCache();
        _endpoint = mode == 2
            ? new Uri(address.TrimEnd('/') + "/icons/")
            : new Uri("https://icons.bitwarden.net/");
    }

    internal static void ClearCache()
    {
        _requests.Cancel();
        _requests.Dispose();
        _requests = new CancellationTokenSource();
        Cache.Clear();
    }

    internal static Task<byte[]?> FetchAsync(string? host)
    {
        if (!Enabled || _endpoint is null || string.IsNullOrEmpty(host) ||
            Uri.CheckHostName(host) != UriHostNameType.Dns || !host.Contains('.'))
            return Task.FromResult<byte[]?>(null);
        var url = new Uri(_endpoint, Uri.EscapeDataString(host) + "/icon.png");
        if (Cache.TryGetValue(url, out var cached)) return cached;
        if (Cache.Count >= 256) Cache.Remove(Cache.Keys.First());
        return Cache[url] = FetchAsync(url, _requests.Token);
    }

    private static async Task<byte[]?> FetchAsync(Uri url, CancellationToken token)
    {
        try
        {
            using var response = await Client.GetAsync(url, token).ConfigureAwait(false);
            if (response.IsSuccessStatusCode &&
                response.Content.Headers.ContentType?.MediaType?.StartsWith("image/", StringComparison.OrdinalIgnoreCase) == true)
                return await response.Content.ReadAsByteArrayAsync(token).ConfigureAwait(false);
        }
        catch (Exception error) when (error is HttpRequestException or OperationCanceledException) { }
        return null;
    }

    private static bool ReadPreference()
    {
        try
        {
            return !ApplicationData.Current.LocalSettings.Values.TryGetValue(PreferenceKey, out var value) ||
                value is not false;
        }
        catch (Exception error) when (error is COMException or InvalidOperationException) { return true; }
    }
}
