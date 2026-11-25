using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bytewarden.WinUI.Pages;

public sealed partial class GeneratorPage : Page
{
    private readonly DispatcherTimer _regenerationTimer;
    private bool _ready;
    private double _lastMinimumNumbers = 1;
    private double _lastMinimumSpecial = 1;
    private bool _updatingMinimums;
    private string? _settingsScope;
    private bool _settingsLoaded;
    private GeneratorSettings _settings = new();
    private GeneratorSettings? _pendingSettings;

    public static void ResetSavedAlgorithm(string scope)
    {
        try { GeneratorSettings.ResetSelectedType(ApplicationData.Current.LocalSettings.Values, scope); }
        catch (Exception error) when (IsSettingsUnavailable(error)) { }
    }

    public GeneratorPage()
    {
        InitializeComponent();
        _regenerationTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(150) };
        _regenerationTimer.Tick += RegenerationTimer_Tick;
        RefreshSettingsScope();
        Loaded += (_, _) =>
        {
            AccountSettingsScope.SignedOut += Account_SignedOut;
            RefreshSettingsScope();
            GenerateCurrent();
        };
        Unloaded += (_, _) =>
        {
            AccountSettingsScope.SignedOut -= Account_SignedOut;
            ClearSensitiveState();
        };
    }

    public void ClearSensitiveState()
    {
        PersistPendingSettings();
        _regenerationTimer.Stop();
        GeneratedPassword.Text = string.Empty;
    }

    public void RefreshSettingsScope() => RefreshSettingsScopeIfChanged();

    private bool RefreshSettingsScopeIfChanged()
    {
        var scope = AccountSettingsScope.Current;
        if (_settingsLoaded && _settingsScope == scope) return false;
        PersistPendingSettings();
        _regenerationTimer.Stop();
        GeneratedPassword.Text = string.Empty;
        GeneratorNotice.IsOpen = false;
        _settingsScope = scope;
        try { _settings = GeneratorSettings.Load(ApplicationData.Current.LocalSettings.Values, scope); }
        catch (Exception error) when (IsSettingsUnavailable(error)) { _settings = new(); }
        _ready = false;
        LengthSlider.Value = _settings.PasswordLength;
        Uppercase.IsOn = _settings.Uppercase;
        Lowercase.IsOn = _settings.Lowercase;
        Numbers.IsOn = _settings.Numbers;
        Special.IsOn = _settings.Special;
        AvoidAmbiguous.IsOn = _settings.AvoidAmbiguous;
        MinimumNumbers.Value = _settings.MinimumNumbers;
        MinimumSpecial.Value = _settings.MinimumSpecial;
        _lastMinimumNumbers = Math.Max(_settings.MinimumNumbers, 1);
        _lastMinimumSpecial = Math.Max(_settings.MinimumSpecial, 1);
        PassphraseWords.Value = _settings.PassphraseWords;
        WordSeparator.Text = _settings.WordSeparator;
        CapitalizeWords.IsOn = _settings.CapitalizeWords;
        IncludeNumber.IsOn = _settings.IncludeNumber;
        CredentialType.SelectedIndex = _settings.SelectedType;
        UpdateCredentialTypePresentation();
        _settingsLoaded = _ready = true;
        return true;
    }

    private void Account_SignedOut(string scope)
    {
        void Refresh()
        {
            if (_settingsScope != scope) return;
            // A pending edit may have been captured just before logout was requested.
            if (_pendingSettings is not null) _pendingSettings = _pendingSettings with { SelectedType = 0 };
            ClearSensitiveState();
            RefreshSettingsScope();
        }
        if (DispatcherQueue.HasThreadAccess) Refresh();
        else DispatcherQueue.TryEnqueue(Refresh);
    }

    private static bool IsSettingsUnavailable(Exception error) =>
        error is COMException or InvalidOperationException or ArgumentException;

    private void PersistPendingSettings()
    {
        if (_pendingSettings is not { } settings) return;
        _pendingSettings = null;
        if (_settingsScope is not null && _settingsScope != AccountSettingsScope.Current)
            settings = settings with { SelectedType = 0 };
        _settings = settings.Normalize();
        try { _settings.Save(ApplicationData.Current.LocalSettings.Values, _settingsScope); }
        catch (Exception error) when (IsSettingsUnavailable(error)) { }
    }

    private GeneratorSettings CaptureSettings() => new()
    {
        SelectedType = IsPassphraseSelected ? 1 : 0,
        PasswordLength = (int)LengthSlider.Value,
        Uppercase = Uppercase.IsOn,
        Lowercase = Lowercase.IsOn,
        Numbers = Numbers.IsOn,
        Special = Special.IsOn,
        AvoidAmbiguous = AvoidAmbiguous.IsOn,
        MinimumNumbers = FiniteCount(MinimumNumbers.Value, _settings.MinimumNumbers),
        MinimumSpecial = FiniteCount(MinimumSpecial.Value, _settings.MinimumSpecial),
        PassphraseWords = FiniteCount(PassphraseWords.Value, _settings.PassphraseWords),
        WordSeparator = WordSeparator.Text,
        CapitalizeWords = CapitalizeWords.IsOn,
        IncludeNumber = IncludeNumber.IsOn,
    };

    private static int FiniteCount(double value, int fallback) =>
        double.IsFinite(value) ? (int)Math.Clamp(value, 0, 128) : fallback;

    private void LengthSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (LengthLabel is not null)
        {
            LengthLabel.Text = ((int)e.NewValue).ToString();
        }
        ScheduleRegeneration();
    }

    private void Generate_Click(object sender, RoutedEventArgs e)
    {
        RefreshSettingsScope();
        GenerateCurrent();
    }

    private void GenerateCurrent()
    {
        if (RefreshSettingsScopeIfChanged()) return;
        _regenerationTimer.Stop();
        PersistPendingSettings();
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
            GeneratedPassword.Text = string.Empty;
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
        ClearSensitiveState();
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

        try { ClipboardCleaner.Copy(GeneratedPassword.Text); }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
        {
            GeneratorNotice.Title = "无法复制";
            GeneratorNotice.Message = "剪贴板暂时不可用，请重试。";
            GeneratorNotice.Severity = InfoBarSeverity.Error;
            GeneratorNotice.IsOpen = true;
            return;
        }
        GeneratorNotice.Title = "已复制";
        GeneratorNotice.Message = IsPassphraseSelected
            ? "密码短语已复制。30 秒后会清除本应用仍然拥有的剪贴板内容。"
            : "密码已复制。30 秒后会清除本应用仍然拥有的剪贴板内容。";
        GeneratorNotice.Severity = InfoBarSeverity.Success;
        GeneratorNotice.IsOpen = true;
    }

    private static uint ToCount(double? value) => value is double number && double.IsFinite(number) && number > 0
        ? (uint)Math.Min(number, 128) : 0;

    private bool IsPassphraseSelected => CredentialType.SelectedIndex == 1;

    private void CredentialType_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_ready) return;
        UpdateCredentialTypePresentation();
        ScheduleRegeneration();
    }

    private void UpdateCredentialTypePresentation()
    {
        var showPassphrase = IsPassphraseSelected;
        PasswordOptions.Visibility = showPassphrase ? Visibility.Collapsed : Visibility.Visible;
        PassphraseOptions.Visibility = showPassphrase ? Visibility.Visible : Visibility.Collapsed;
        GeneratedLabel.Text = showPassphrase ? "生成的密码短语" : "生成的密码";
        GenerateButton.Content = showPassphrase ? "生成密码短语" : "生成密码";
    }

    private void PasswordOption_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_ready || _updatingMinimums)
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
        if (!_ready) return;
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
        if (!_ready)
        {
            return;
        }

        if (RefreshSettingsScopeIfChanged()) return;
        _pendingSettings = CaptureSettings();

        _regenerationTimer.Stop();
        _regenerationTimer.Start();
    }

    private void RegenerationTimer_Tick(object? sender, object e)
    {
        GenerateCurrent();
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
