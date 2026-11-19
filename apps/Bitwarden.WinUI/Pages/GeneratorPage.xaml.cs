using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Windows.ApplicationModel.DataTransfer;

namespace Bitwarden_WinUI.Pages;

public sealed partial class GeneratorPage : Page
{
    private DispatcherTimer? _regenerationTimer;
    private double _lastMinimumNumbers = 1;
    private double _lastMinimumSpecial = 1;
    private bool _updatingMinimums;

    public GeneratorPage()
    {
        InitializeComponent();
        _regenerationTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(150) };
        _regenerationTimer.Tick += RegenerationTimer_Tick;
        CredentialType.SelectedIndex = 0;
        GenerateCurrent();
    }

    private void LengthSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (LengthLabel is not null)
        {
            LengthLabel.Text = ((int)e.NewValue).ToString();
        }
        ScheduleRegeneration();
    }

    private void Generate_Click(object sender, RoutedEventArgs e) => GenerateCurrent();

    private void GenerateCurrent()
    {
        try
        {
            GeneratedPassword.Text = IsPassphraseSelected
                ? RustCore.GeneratePassphrase(
                    ToCount(PassphraseWords.Value),
                    WordSeparator.Text,
                    CapitalizeWords.IsOn,
                    IncludeNumber.IsOn)
                : RustCore.GeneratePassword(
                    (uint)LengthSlider.Value,
                    Uppercase.IsOn,
                    Lowercase.IsOn,
                    Numbers.IsOn,
                    Special.IsOn,
                    AvoidAmbiguous.IsOn,
                    ToCount(MinimumNumbers.Value),
                    ToCount(MinimumSpecial.Value));
            GeneratorNotice.Title = "已生成";
            GeneratorNotice.Severity = InfoBarSeverity.Success;
            GeneratorNotice.IsOpen = false;
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException or InvalidOperationException)
        {
            GeneratorNotice.Title = "无法生成密码";
            GeneratorNotice.Severity = InfoBarSeverity.Error;
            GeneratorNotice.Message = error is InvalidOperationException
                ? IsPassphraseSelected
                    ? "密码短语需要 3–20 个单词，请检查选项。"
                    : "当前选项无法在所选长度内满足，请增加长度或减少最少字符数。"
                : "Rust 核心 DLL 未加载；请从仓库根目录构建应用。";
            GeneratorNotice.IsOpen = true;
        }
    }

    private void Clear_Click(object sender, RoutedEventArgs e)
    {
        GeneratedPassword.Text = string.Empty;
        GeneratorNotice.Title = "无法生成密码";
        GeneratorNotice.Severity = InfoBarSeverity.Error;
        GeneratorNotice.IsOpen = false;
    }

    private void Copy_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(GeneratedPassword.Text))
        {
            GeneratorNotice.Title = IsPassphraseSelected ? "没有可复制的密码短语" : "没有可复制的密码";
            GeneratorNotice.Message = IsPassphraseSelected ? "请先生成密码短语。" : "请先生成密码。";
            GeneratorNotice.Severity = InfoBarSeverity.Warning;
            GeneratorNotice.IsOpen = true;
            return;
        }

        var package = new DataPackage();
        package.SetText(GeneratedPassword.Text);
        Clipboard.SetContent(package);
        GeneratorNotice.Title = "已复制";
        GeneratorNotice.Message = IsPassphraseSelected
            ? "密码短语已复制到剪贴板。自动清理将在后续迭代接入。"
            : "密码已复制到剪贴板。自动清理将在后续迭代接入。";
        GeneratorNotice.Severity = InfoBarSeverity.Success;
        GeneratorNotice.IsOpen = true;
    }

    private static uint ToCount(double? value) => value is double number && number > 0 ? (uint)number : 0;

    private bool IsPassphraseSelected => CredentialType.SelectedIndex == 1;

    private void CredentialType_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        var showPassphrase = IsPassphraseSelected;
        PasswordOptions.Visibility = showPassphrase ? Visibility.Collapsed : Visibility.Visible;
        PassphraseOptions.Visibility = showPassphrase ? Visibility.Visible : Visibility.Collapsed;
        GeneratedLabel.Text = showPassphrase ? "生成的密码短语" : "生成的密码";
        GenerateButton.Content = showPassphrase ? "生成密码短语" : "生成密码";
        ScheduleRegeneration();
    }

    private void PasswordOption_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updatingMinimums)
        {
            return;
        }

        if (ReferenceEquals(sender, Numbers))
        {
            UpdateMinimumFromToggle(Numbers, MinimumNumbers, ref _lastMinimumNumbers);
        }
        else if (ReferenceEquals(sender, Special))
        {
            UpdateMinimumFromToggle(Special, MinimumSpecial, ref _lastMinimumSpecial);
        }

        ScheduleRegeneration();
    }

    private void PasswordOption_ValueChanged(object sender, NumberBoxValueChangedEventArgs e)
    {
        if (!_updatingMinimums && sender is NumberBox numberBox)
        {
            if (ReferenceEquals(numberBox, MinimumNumbers))
            {
                if (e.NewValue > 0)
                {
                    _lastMinimumNumbers = e.NewValue;
                }
                _updatingMinimums = true;
                Numbers.IsOn = e.NewValue > 0;
                _updatingMinimums = false;
            }
            else
            {
                if (e.NewValue > 0)
                {
                    _lastMinimumSpecial = e.NewValue;
                }
                _updatingMinimums = true;
                Special.IsOn = e.NewValue > 0;
                _updatingMinimums = false;
            }
        }

        ScheduleRegeneration();
    }

    private void PassphraseOption_ValueChanged(object sender, NumberBoxValueChangedEventArgs e) => ScheduleRegeneration();

    private void PassphraseOption_Toggled(object sender, RoutedEventArgs e) => ScheduleRegeneration();

    private void PassphraseOption_TextChanged(object sender, TextChangedEventArgs e) => ScheduleRegeneration();

    private void ScheduleRegeneration()
    {
        if (_regenerationTimer is null)
        {
            return;
        }

        _regenerationTimer.Stop();
        _regenerationTimer.Start();
    }

    private void RegenerationTimer_Tick(object? sender, object e)
    {
        _regenerationTimer?.Stop();
        GenerateCurrent();
    }

    private void Page_Unloaded(object sender, RoutedEventArgs e)
    {
        if (_regenerationTimer is not null)
        {
            _regenerationTimer.Stop();
            _regenerationTimer.Tick -= RegenerationTimer_Tick;
            _regenerationTimer = null;
        }
    }

    private void UpdateMinimumFromToggle(ToggleSwitch toggle, NumberBox minimum, ref double previous)
    {
        if (toggle.IsOn)
        {
            if (minimum.Value > 0)
            {
                previous = minimum.Value;
            }

            _updatingMinimums = true;
            minimum.Value = Math.Max(previous, 1);
            _updatingMinimums = false;
        }
        else if (minimum.Value > 0)
        {
            previous = minimum.Value;
            _updatingMinimums = true;
            minimum.Value = 0;
            _updatingMinimums = false;
        }
    }
}
