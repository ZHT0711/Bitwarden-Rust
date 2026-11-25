using Bytewarden.WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using System.Runtime.InteropServices;
using Windows.Storage;

namespace Bytewarden.WinUI.Pages;

public sealed record GeneratorHistoryItem(string Value, string Meta);

public sealed partial class GeneratorPage : Page
{
    private const int MaxHistoryEntries = 100;
    private readonly DispatcherTimer _regenerationTimer;
    private readonly List<GeneratorHistoryItem> _history = [];
    private CancellationTokenSource? _usernameOperation;
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
        UsernameTypeChoice.SelectedIndex = 0;
        ForwarderServiceChoice.SelectedIndex = 0;
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
        CancelUsernameOperation();
        GeneratedPassword.Text = string.Empty;
        PasswordStrengthLabel.Text = string.Empty;
        PasswordStrengthLabel.Visibility = Visibility.Collapsed;
        ForwarderApiKey.Password = string.Empty;
        _history.Clear();
        HistoryList.ItemsSource = null;
        HistoryExpander.Visibility = Visibility.Collapsed;
    }

    public void RefreshSettingsScope() => RefreshSettingsScopeIfChanged();

    private bool RefreshSettingsScopeIfChanged()
    {
        var scope = AccountSettingsScope.Current;
        if (_settingsLoaded && _settingsScope == scope) return false;
        PersistPendingSettings();
        _regenerationTimer.Stop();
        CancelUsernameOperation();
        GeneratedPassword.Text = string.Empty;
        PasswordStrengthLabel.Text = string.Empty;
        PasswordStrengthLabel.Visibility = Visibility.Collapsed;
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
        UpdateUsernameOptionVisibility();
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
        if (IsUsernameSelected) _ = GenerateUsernameAsync();
        else GenerateCurrent();
    }

    private void GenerateCurrent()
    {
        if (RefreshSettingsScopeIfChanged()) return;
        _regenerationTimer.Stop();
        PersistPendingSettings();
        if (IsUsernameSelected)
        {
            _ = GenerateUsernameAsync();
            return;
        }
        try
        {
            var generated = IsPassphraseSelected
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
            GeneratedPassword.Text = generated;
            UpdateStrength(generated);
            RecordHistory(generated, IsPassphraseSelected ? "密码短语" : "密码");
            GeneratorNotice.Title = "已生成";
            GeneratorNotice.Severity = InfoBarSeverity.Success;
            GeneratorNotice.IsOpen = false;
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException or InvalidOperationException)
        {
            GeneratedPassword.Text = string.Empty;
            PasswordStrengthLabel.Visibility = Visibility.Collapsed;
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

    private async Task GenerateUsernameAsync()
    {
        CancelUsernameOperation();
        var cancellation = _usernameOperation = new CancellationTokenSource();
        var token = cancellation.Token;
        var mode = (UsernameTypeChoice.SelectedItem as ComboBoxItem)?.Tag as string ?? "word";
        var capitalize = UsernameCapitalize.IsOn;
        var includeNumber = UsernameIncludeNumber.IsOn;
        var subEmail = SubaddressEmail.Text.Trim();
        var catchDomain = CatchallDomain.Text.Trim();
        var useWebsite = UsernameUseWebsiteToggle.IsOn;
        var website = UsernameWebsite.Text.Trim();
        var service = (ForwarderServiceChoice.SelectedItem as ComboBoxItem)?.Tag as string ?? "addyIo";
        var apiKey = ForwarderApiKey.Password;
        var forwarderDomain = ForwarderDomain.Text.Trim();
        var forwarderBaseUrl = ForwarderBaseUrl.Text.Trim();

        if (mode == "subaddress" && string.IsNullOrEmpty(subEmail))
        {
            ShowUsernameError("请先填写基础邮箱地址。");
            return;
        }
        if (mode == "catchall" && string.IsNullOrEmpty(catchDomain))
        {
            ShowUsernameError("请先填写邮箱域名。");
            return;
        }
        if (mode == "forwarded" && string.IsNullOrEmpty(apiKey))
        {
            ShowUsernameError("请填写所选转发服务的 API Key / Token。");
            return;
        }
        if (mode == "forwarded" && service is "addyIo" or "forwardEmail" && string.IsNullOrEmpty(forwarderDomain))
        {
            ShowUsernameError("该转发服务需要填写域名。");
            return;
        }

        UsernameProgress.IsActive = true;
        UsernameProgress.Visibility = Visibility.Visible;
        GenerateButton.IsEnabled = false;
        GeneratorNotice.IsOpen = false;
        try
        {
            var generated = await Task.Run(() => mode switch
            {
                "subaddress" => RustCore.GenerateUsernameSubaddress(subEmail, useWebsite ? website : null, token),
                "catchall" => RustCore.GenerateUsernameCatchall(catchDomain, useWebsite ? website : null, token),
                "forwarded" => RustCore.GenerateUsernameForwarded(service, apiKey, forwarderDomain, forwarderBaseUrl, website, token),
                _ => RustCore.GenerateUsernameWord(capitalize, includeNumber, token),
            }, token);
            if (!ReferenceEquals(_usernameOperation, cancellation) || token.IsCancellationRequested) return;
            GeneratedPassword.Text = generated;
            PasswordStrengthLabel.Visibility = Visibility.Collapsed;
            RecordHistory(generated, "用户名");
        }
        catch (OperationCanceledException) { }
        catch (VaultException error)
        {
            if (ReferenceEquals(_usernameOperation, cancellation))
            {
                ShowUsernameError(error.Code == "validation"
                    ? "请检查转发服务配置、API Key 或域名参数。"
                    : "生成用户名失败，请检查网络连接和服务端点后重试。");
            }
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            if (ReferenceEquals(_usernameOperation, cancellation))
                ShowUsernameError("Rust 核心 DLL 未加载；请从仓库根目录构建应用。");
        }
        finally
        {
            apiKey = string.Empty;
            if (ReferenceEquals(_usernameOperation, cancellation))
            {
                _usernameOperation = null;
                UsernameProgress.IsActive = false;
                UsernameProgress.Visibility = Visibility.Collapsed;
                GenerateButton.IsEnabled = true;
            }
            cancellation.Dispose();
        }
    }

    private void ShowUsernameError(string message)
    {
        GeneratorNotice.Title = "无法生成用户名";
        GeneratorNotice.Severity = InfoBarSeverity.Error;
        GeneratorNotice.Message = message;
        GeneratorNotice.IsOpen = true;
    }

    private void CancelUsernameOperation()
    {
        _usernameOperation?.Cancel();
        _usernameOperation = null;
        if (UsernameProgress is not null)
        {
            UsernameProgress.IsActive = false;
            UsernameProgress.Visibility = Visibility.Collapsed;
        }
        if (GenerateButton is not null) GenerateButton.IsEnabled = true;
    }

    private void UpdateStrength(string value)
    {
        if (IsUsernameSelected || string.IsNullOrEmpty(value))
        {
            PasswordStrengthLabel.Text = string.Empty;
            PasswordStrengthLabel.Visibility = Visibility.Collapsed;
            return;
        }
        try
        {
            var score = RustCore.CheckPasswordStrength(value);
            if (score >= 0)
            {
                PasswordStrengthLabel.Text = $"密码强度：{HomePage.PasswordStrengthLabel(score)}";
                PasswordStrengthLabel.Visibility = Visibility.Visible;
            }
            else
            {
                PasswordStrengthLabel.Visibility = Visibility.Collapsed;
            }
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            PasswordStrengthLabel.Visibility = Visibility.Collapsed;
        }
    }

    private void RecordHistory(string value, string kind)
    {
        if (string.IsNullOrEmpty(value)) return;
        if (_history.Count > 0 && _history[0].Value == value) return;
        _history.Insert(0, new GeneratorHistoryItem(value, $"{kind} · {DateTime.Now:HH:mm:ss}"));
        if (_history.Count > MaxHistoryEntries) _history.RemoveRange(MaxHistoryEntries, _history.Count - MaxHistoryEntries);
        HistoryList.ItemsSource = null;
        HistoryList.ItemsSource = _history.ToArray();
        HistoryExpander.Visibility = Visibility.Visible;
    }

    private void ClearHistory_Click(object sender, RoutedEventArgs e)
    {
        _history.Clear();
        HistoryList.ItemsSource = null;
        HistoryExpander.Visibility = Visibility.Collapsed;
    }

    private void CopyHistory_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: string value } && !string.IsNullOrEmpty(value))
            CopyValue(value, "内容");
    }

    private void Clear_Click(object sender, RoutedEventArgs e)
    {
        CancelUsernameOperation();
        _regenerationTimer.Stop();
        GeneratedPassword.Text = string.Empty;
        PasswordStrengthLabel.Text = string.Empty;
        PasswordStrengthLabel.Visibility = Visibility.Collapsed;
        GeneratorNotice.IsOpen = false;
    }

    private void Copy_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(GeneratedPassword.Text))
        {
            GeneratorNotice.Title = IsPassphraseSelected ? "没有可复制的密码短语" : IsUsernameSelected ? "没有可复制的用户名" : "没有可复制的密码";
            GeneratorNotice.Message = IsPassphraseSelected ? "请先生成密码短语。" : IsUsernameSelected ? "请先生成用户名。" : "请先生成密码。";
            GeneratorNotice.Severity = InfoBarSeverity.Warning;
            GeneratorNotice.IsOpen = true;
            return;
        }

        CopyValue(GeneratedPassword.Text, IsPassphraseSelected ? "密码短语" : IsUsernameSelected ? "用户名" : "密码");
    }

    private void CopyValue(string value, string label)
    {
        try { ClipboardCleaner.Copy(value); }
        catch (Exception error) when (error is COMException or InvalidOperationException)
        {
            GeneratorNotice.Title = "无法复制";
            GeneratorNotice.Message = "剪贴板暂时不可用，请重试。";
            GeneratorNotice.Severity = InfoBarSeverity.Error;
            GeneratorNotice.IsOpen = true;
            return;
        }
        var delay = DesktopPreferences.ReadClipboardDelay();
        var clearNote = delay == 0
            ? "未启用定时清除，锁定或退出时仍会尝试清除本应用拥有的剪贴板内容。"
            : $"{delay} 秒后会清除本应用仍然拥有的剪贴板内容。";
        GeneratorNotice.Title = "已复制";
        GeneratorNotice.Message = $"{label}已复制。{clearNote}";
        GeneratorNotice.Severity = InfoBarSeverity.Success;
        GeneratorNotice.IsOpen = true;
    }

    private static uint ToCount(double? value) => value is double number && double.IsFinite(number) && number > 0
        ? (uint)Math.Min(number, 128) : 0;

    private bool IsPassphraseSelected => CredentialType.SelectedIndex == 1;
    private bool IsUsernameSelected => CredentialType.SelectedIndex == 2;

    private void CredentialType_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_ready) return;
        UpdateCredentialTypePresentation();
        if (IsUsernameSelected)
        {
            _regenerationTimer.Stop();
            GeneratedPassword.Text = string.Empty;
            PasswordStrengthLabel.Visibility = Visibility.Collapsed;
            if ((UsernameTypeChoice.SelectedItem as ComboBoxItem)?.Tag as string == "word")
                _ = GenerateUsernameAsync();
        }
        else
        {
            CancelUsernameOperation();
            ScheduleRegeneration();
        }
    }

    private void UpdateCredentialTypePresentation()
    {
        var showPassphrase = IsPassphraseSelected;
        var showUsername = IsUsernameSelected;
        PasswordOptions.Visibility = !showPassphrase && !showUsername ? Visibility.Visible : Visibility.Collapsed;
        PassphraseOptions.Visibility = showPassphrase ? Visibility.Visible : Visibility.Collapsed;
        UsernameOptions.Visibility = showUsername ? Visibility.Visible : Visibility.Collapsed;
        GeneratedLabel.Text = showPassphrase ? "生成的密码短语" : showUsername ? "生成的用户名" : "生成的密码";
        GenerateButton.Content = showPassphrase ? "生成密码短语" : showUsername ? "生成用户名" : "生成密码";
        GeneratedPassword.PlaceholderText = showPassphrase ? "点击“生成密码短语”" : showUsername ? "点击“生成用户名”" : "点击“生成密码”";
    }

    private void UsernameTypeChoice_Changed(object sender, SelectionChangedEventArgs e) => UpdateUsernameOptionVisibility();
    private void UsernameUseWebsiteToggle_Toggled(object sender, RoutedEventArgs e) => UpdateUsernameOptionVisibility();
    private void ForwarderServiceChoice_Changed(object sender, SelectionChangedEventArgs e) => UpdateUsernameOptionVisibility();

    private void UpdateUsernameOptionVisibility()
    {
        if (UsernameTypeChoice is null) return;
        var mode = (UsernameTypeChoice.SelectedItem as ComboBoxItem)?.Tag as string ?? "word";
        UsernameWordPanel.Visibility = mode == "word" ? Visibility.Visible : Visibility.Collapsed;
        SubaddressEmail.Visibility = mode == "subaddress" ? Visibility.Visible : Visibility.Collapsed;
        CatchallDomain.Visibility = mode == "catchall" ? Visibility.Visible : Visibility.Collapsed;
        UsernameUseWebsiteToggle.Visibility = mode is "subaddress" or "catchall" ? Visibility.Visible : Visibility.Collapsed;
        UsernameForwarderPanel.Visibility = mode == "forwarded" ? Visibility.Visible : Visibility.Collapsed;
        UsernameWebsite.Visibility = mode == "forwarded" || (mode is "subaddress" or "catchall" && UsernameUseWebsiteToggle.IsOn)
            ? Visibility.Visible : Visibility.Collapsed;
        if (mode == "forwarded")
        {
            var service = (ForwarderServiceChoice.SelectedItem as ComboBoxItem)?.Tag as string ?? "addyIo";
            ForwarderDomain.Visibility = service is "addyIo" or "forwardEmail" ? Visibility.Visible : Visibility.Collapsed;
            ForwarderBaseUrl.Visibility = service is "addyIo" or "simpleLogin" ? Visibility.Visible : Visibility.Collapsed;
            ForwarderBaseUrl.PlaceholderText = service == "simpleLogin" ? "https://app.simplelogin.io" : "https://app.addy.io";
        }
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
        if (!_ready || IsUsernameSelected)
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
