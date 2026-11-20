using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Bitwarden_WinUI.Controls;

public sealed partial class WebsiteIcon : UserControl
{
    public static readonly DependencyProperty HostProperty =
        DependencyProperty.Register(nameof(Host), typeof(string), typeof(WebsiteIcon),
            new PropertyMetadata(null, (sender, _) => ((WebsiteIcon)sender).Refresh()));

    private readonly FontIcon _fallback = new() { Glyph = "\uE774", FontSize = 17 };
    private readonly Image _image = new() { Stretch = Stretch.Uniform };
    private int _version;
    public string? Host
    {
        get => (string?)GetValue(HostProperty);
        set => SetValue(HostProperty, value);
    }

    public WebsiteIcon()
    {
        Width = Height = 24;
        var contents = new Grid();
        contents.Children.Add(_fallback);
        contents.Children.Add(_image);
        Content = contents;
        Loaded += (_, _) => Refresh();
        Unloaded += (_, _) =>
        {
            _version++;
            ResetImage();
        };
    }

    private async void Refresh()
    {
        var version = ++_version;
        ResetImage();
        if (!IsLoaded) return;

        var bytes = await WebsiteIcons.FetchAsync(Host);
        if (bytes is null || version != _version) return;
        try
        {
            using var stream = new MemoryStream(bytes);
            using var input = stream.AsRandomAccessStream();
            var bitmap = new BitmapImage { DecodePixelWidth = 48 };
            await bitmap.SetSourceAsync(input);
            if (version != _version) return;
            _image.Source = bitmap;
            _fallback.Visibility = Visibility.Collapsed;
        }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or ArgumentException) { }
    }

    private void ResetImage()
    {
        _image.Source = null;
        _fallback.Visibility = Visibility.Visible;
    }
}
