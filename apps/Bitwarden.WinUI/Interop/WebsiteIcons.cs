using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage;

namespace Bitwarden_WinUI.Interop;

internal static class WebsiteIcons
{
    private const string PreferenceKey = "ShowWebsiteIcons";
    private const int MaxDecodedBitmaps = 128;
    private const int MaxNegativeHosts = 256;
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
    private static readonly Dictionary<Uri, Task<byte[]?>> InFlight = new();
    private static readonly Dictionary<Uri, LinkedListNode<(Uri Url, BitmapImage Bitmap)>> Decoded = new();
    private static readonly LinkedList<(Uri Url, BitmapImage Bitmap)> DecodedLru = new();
    private static readonly HashSet<Uri> Missed = new();
    private static CancellationTokenSource _requests = new();
    private static Uri? _endpoint;
    private static int _generation;

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

    internal static void ClearServer()
    {
        ClearCache();
        _endpoint = null;
    }

    internal static void ClearCache()
    {
        _generation++;
        _requests.Cancel();
        _requests.Dispose();
        _requests = new CancellationTokenSource();
        InFlight.Clear();
        Decoded.Clear();
        DecodedLru.Clear();
        Missed.Clear();
    }

    internal static Uri? TryBuildUri(string? host)
    {
        if (!Enabled || _endpoint is null || string.IsNullOrEmpty(host) ||
            Uri.CheckHostName(host) != UriHostNameType.Dns || !host.Contains('.'))
            return null;
        return new Uri(_endpoint, Uri.EscapeDataString(host) + "/icon.png");
    }

    internal static bool TryGetCachedBitmap(Uri url, out BitmapImage? bitmap, out bool knownMiss)
    {
        if (Decoded.TryGetValue(url, out var node))
        {
            if (node != DecodedLru.First)
            {
                DecodedLru.Remove(node);
                DecodedLru.AddFirst(node);
            }
            bitmap = node.Value.Bitmap;
            knownMiss = false;
            return true;
        }
        bitmap = null;
        knownMiss = Missed.Contains(url);
        return knownMiss;
    }

    internal static async Task<BitmapImage?> GetBitmapAsync(Uri url)
    {
        if (TryGetCachedBitmap(url, out var cached, out var knownMiss)) return knownMiss ? null : cached;
        var generation = _generation;
        if (!InFlight.TryGetValue(url, out var fetchTask))
        {
            fetchTask = FetchBytesAsync(url, _requests.Token);
            InFlight[url] = fetchTask;
        }

        byte[]? bytes;
        try { bytes = await fetchTask; }
        finally
        {
            if (generation == _generation) InFlight.Remove(url);
        }

        if (generation != _generation) return null;
        if (bytes is null || bytes.Length == 0)
        {
            RememberMiss(url);
            return null;
        }
        if (TryGetCachedBitmap(url, out var alreadyDecoded, out _)) return alreadyDecoded;

        try
        {
            using var stream = new MemoryStream(bytes, writable: false);
            using var input = stream.AsRandomAccessStream();
            var bitmap = new BitmapImage { DecodePixelWidth = 48 };
            await bitmap.SetSourceAsync(input);
            if (generation != _generation) return null;
            RememberDecoded(url, bitmap);
            return bitmap;
        }
        catch (Exception error) when (error is COMException or ArgumentException)
        {
            if (generation == _generation) RememberMiss(url);
            return null;
        }
    }

    private static void RememberDecoded(Uri url, BitmapImage bitmap)
    {
        Missed.Remove(url);
        if (Decoded.TryGetValue(url, out var existing))
        {
            DecodedLru.Remove(existing);
            Decoded.Remove(url);
        }
        else if (Decoded.Count >= MaxDecodedBitmaps && DecodedLru.Last is { } oldest)
        {
            Decoded.Remove(oldest.Value.Url);
            DecodedLru.RemoveLast();
        }
        var node = DecodedLru.AddFirst((url, bitmap));
        Decoded[url] = node;
    }

    private static void RememberMiss(Uri url)
    {
        if (Missed.Count >= MaxNegativeHosts) Missed.Clear();
        Missed.Add(url);
    }

    private static async Task<byte[]?> FetchBytesAsync(Uri url, CancellationToken token)
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
