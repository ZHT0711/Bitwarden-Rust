using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Bitwarden_WinUI.Controls;

public sealed partial class WebsiteIcon : Grid
{
    public static readonly DependencyProperty HostProperty =
        DependencyProperty.Register(nameof(Host), typeof(string), typeof(WebsiteIcon),
            new PropertyMetadata(null, (sender, args) =>
            {
                if (!string.Equals(args.OldValue as string, args.NewValue as string, StringComparison.Ordinal))
                    ((WebsiteIcon)sender).Refresh();
            }));

    private readonly FontIcon _fallback = new() { Glyph = "\uE774", FontSize = 16 };
    private Image? _image;
    private int _version;

    public string? Host
    {
        get => (string?)GetValue(HostProperty);
        set => SetValue(HostProperty, value);
    }

    public WebsiteIcon()
    {
        Width = Height = 24;
        Children.Add(_fallback);
        Loaded += (_, _) => Refresh();
        Unloaded += (_, _) =>
        {
            _version++;
            ResetImage();
        };
    }

    private void Refresh()
    {
        var version = ++_version;
        if (!IsLoaded)
        {
            ResetImage();
            return;
        }

        var url = WebsiteIcons.TryBuildUri(Host);
        if (url is null)
        {
            ResetImage();
            return;
        }

        if (WebsiteIcons.TryGetCachedBitmap(url, out var cached, out var knownMiss))
        {
            if (cached is not null && !knownMiss) ApplyBitmap(cached);
            else ResetImage();
            return;
        }

        ResetImage();
        _ = LoadAsync(url, version);
    }

    private async Task LoadAsync(Uri url, int version)
    {
        var bitmap = await WebsiteIcons.GetBitmapAsync(url);
        if (bitmap is null || version != _version || !IsLoaded) return;
        ApplyBitmap(bitmap);
    }

    private void ApplyBitmap(BitmapImage bitmap)
    {
        if (_image is null)
        {
            _image = new Image { Stretch = Stretch.Uniform };
            Children.Add(_image);
        }
        _image.Source = bitmap;
        if (_fallback.Visibility != Visibility.Collapsed) _fallback.Visibility = Visibility.Collapsed;
    }

    private void ResetImage()
    {
        if (_image is not null && _image.Source is not null) _image.Source = null;
        if (_fallback.Visibility != Visibility.Visible) _fallback.Visibility = Visibility.Visible;
    }
}
