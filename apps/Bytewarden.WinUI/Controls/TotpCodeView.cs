using Bytewarden.WinUI.Interop;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Bytewarden.WinUI.Controls;

internal sealed class TotpCodeView : IDisposable
{
    private readonly ulong _requestId;
    private readonly DispatcherQueueTimer _timer;
    private readonly TextBlock _value = new() { FontFamily = new FontFamily("Consolas"), FontSize = 22 };
    private readonly TextBlock _countdown = new() { TextWrapping = TextWrapping.Wrap };
    private readonly Button _copy = new() { Content = "复制", VerticalAlignment = VerticalAlignment.Center };
    private readonly ProgressBar _progress = new() { Height = 4 };
    private TotpCode? _code;
    private long _displayedSecond = long.MinValue;
    private bool _refreshing;
    private bool _disposed;

    internal FrameworkElement Element { get; }

    internal TotpCodeView(TotpCode initial, ulong requestId)
    {
        _code = initial;
        _requestId = requestId;
        _timer = DispatcherQueue.GetForCurrentThread().CreateTimer();
        _timer.Interval = TimeSpan.FromMilliseconds(250);
        _timer.Tick += async (_, _) => await UpdateAsync();
        _copy.Click += Copy_Click;
        AutomationProperties.SetName(_copy, "复制验证码");
        AutomationProperties.SetAutomationId(_value, "VaultTotpCode");
        AutomationProperties.SetAutomationId(_countdown, "VaultTotpCountdown");

        var row = new Grid { ColumnSpacing = 8 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        _value.VerticalAlignment = VerticalAlignment.Center;
        row.Children.Add(_value);
        Grid.SetColumn(_copy, 1);
        row.Children.Add(_copy);

        var panel = new StackPanel { Spacing = 6 };
        panel.Children.Add(new TextBlock
        {
            Text = "验证码（TOTP）",
            Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"],
        });
        panel.Children.Add(row);
        panel.Children.Add(_countdown);
        panel.Children.Add(_progress);
        panel.Loaded += async (_, _) =>
        {
            if (_disposed) return;
            _timer.Start();
            await UpdateAsync();
        };
        panel.Unloaded += (_, _) => Dispose();
        Element = panel;
    }

    private async Task UpdateAsync()
    {
        if (_disposed || _refreshing) return;
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        if (_code is { } code && now >= code.ValidFrom && now < code.ValidUntil)
        {
            if (_displayedSecond == now) return;
            _displayedSecond = now;
            var seconds = code.ValidUntil - now;
            var formatted = code.Code.Length > 4 ? code.Code.Insert(code.Code.Length / 2, " ") : code.Code;
            if (_value.Text != formatted) _value.Text = formatted;
            var countdown = $"{seconds} 秒后更新";
            if (_countdown.Text != countdown) _countdown.Text = countdown;
            _progress.Maximum = code.Period;
            _progress.Value = seconds;
            _copy.IsEnabled = true;
            return;
        }

        _code = null;
        _displayedSecond = long.MinValue;
        _value.Text = "—";
        _countdown.Text = "正在更新验证码…";
        _progress.Value = 0;
        _copy.IsEnabled = false;
        _refreshing = true;
        try
        {
            var refreshed = await Task.Run(() => RustCore.RefreshTotp(_requestId));
            if (!_disposed) _code = refreshed;
        }
        catch (VaultException error) when (error.Code == "totpBusy") { }
        catch (VaultException error)
        {
            if (!_disposed)
            {
                _timer.Stop();
                _countdown.Text = error.UserMessage;
            }
        }
        finally { _refreshing = false; }
    }

    private async void Copy_Click(object sender, RoutedEventArgs e)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        if (_disposed || _code is not { } code || now < code.ValidFrom || now >= code.ValidUntil)
        {
            await UpdateAsync();
            return;
        }
        try { ClipboardCleaner.Copy(code.Code); }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
        { _countdown.Text = "剪贴板暂时不可用"; }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _timer.Stop();
        _code = null;
        _value.Text = string.Empty;
        _copy.IsEnabled = false;
        RustCore.CloseVaultDetail(_requestId);
    }
}
