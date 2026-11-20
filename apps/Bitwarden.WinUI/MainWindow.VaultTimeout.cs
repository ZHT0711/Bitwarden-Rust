using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Pages;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;

namespace Bitwarden_WinUI;

public sealed partial class MainWindow
{
    [Flags]
    private enum PendingWindowsTimeout
    {
        None = 0,
        Lock = 1,
        Sleep = 2,
    }

    private WindowsVaultEvents? _windowsVaultEvents;
    private DispatcherQueueTimer? _vaultTimeoutTimer;
    private VaultTimeoutSettings _vaultTimeout = VaultTimeoutSettings.Default;
    private VaultSessionState _observedVaultState;
    private string? _timeoutAccountKey;
    private PendingWindowsTimeout _pendingWindowsTimeout;
    private uint _windowsTimeoutGeneration;
    private long _lastVaultActivity;
    private uint _lastWindowsInput;

    private void InitializeVaultTimeout()
    {
        var dispatcher = DispatcherQueue;
        _windowsVaultEvents = new(WinRT.Interop.WindowNative.GetWindowHandle(this), mode =>
        {
            var generation = _windowsTimeoutGeneration;
            dispatcher.TryEnqueue(DispatcherQueuePriority.High, () =>
            {
                if (generation == _windowsTimeoutGeneration) OnWindowsVaultEvent(mode);
            });
        });
        _vaultTimeoutTimer = dispatcher.CreateTimer();
        _vaultTimeoutTimer.Tick += (_, _) => _ = CheckVaultTimeoutAsync();
        if (Content is UIElement root)
        {
            root.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, _) => RecordVaultActivity()), true);
            root.AddHandler(UIElement.PointerWheelChangedEvent, new PointerEventHandler((_, _) => RecordVaultActivity()), true);
            root.AddHandler(UIElement.KeyDownEvent, new KeyEventHandler((_, _) => RecordVaultActivity()), true);
        }
        Activated += (_, args) =>
        {
            if (args.WindowActivationState == WindowActivationState.Deactivated) return;
            _ = CheckVaultTimeoutAsync();
        };
    }

    private void UseTimeoutSettingsFor(uint serverMode, string address, string email)
    {
        AccountSettingsScope.Begin(serverMode, address, email);
        DiscardPendingWindowsTimeout();
        _timeoutAccountKey = VaultTimeoutSettings.AccountKey(serverMode, address, email);
        _vaultTimeout = VaultTimeoutStore.Load(_timeoutAccountKey);
        if (NavFrame.Content is GeneratorPage generator) generator.RefreshSettingsScope();
    }

    private void RecordVaultActivity()
    {
        _lastVaultActivity = Environment.TickCount64;
        WindowsVaultEvents.TryGetLastInput(out _lastWindowsInput);
    }

    private VaultTimeoutMode EffectiveTimeoutMode => _vaultTimeout.EffectiveMode(
        _windowsVaultEvents?.CanMonitorLock == true, _windowsVaultEvents?.CanMonitorSleep == true);

    private void UpdateVaultTimeoutMonitor(VaultSessionState state)
    {
        if (state != _observedVaultState && state == VaultSessionState.Unlocked) RecordVaultActivity();
        _observedVaultState = state;
        if (state == VaultSessionState.SignedOut)
        {
            AccountSettingsScope.End();
            _timeoutAccountKey = null;
            _vaultTimeout = VaultTimeoutSettings.Default;
        }
        if (NavFrame.Content is SettingsPage page) UpdateTimeoutControls(page);

        _vaultTimeoutTimer?.Stop();
        if (_closed || _accountTransition || _signInDialogOpen || _vaultTimeoutTimer is null) return;
        var mode = EffectiveTimeoutMode;
        if ((mode == VaultTimeoutMode.Inactivity && state == VaultSessionState.Unlocked) ||
            (mode == VaultTimeoutMode.SystemIdle && CanApplyAutomaticTimeout(state)))
        {
            _vaultTimeoutTimer.Interval = mode == VaultTimeoutMode.SystemIdle
                ? TimeSpan.FromSeconds(30) : TimeSpan.FromSeconds(10);
            _vaultTimeoutTimer.Start();
        }
    }

    private void UpdateTimeoutControls(SettingsPage page) =>
        page.ShowVaultTimeout(_vaultTimeout, _observedVaultState == VaultSessionState.Unlocked &&
            !_accountTransition && !_signInDialogOpen, _windowsVaultEvents?.CanMonitorLock == true,
            _windowsVaultEvents?.CanMonitorSleep == true);

    private void SaveVaultTimeout(SettingsPage page, VaultTimeoutSettings settings)
    {
        if (_timeoutAccountKey is null || _observedVaultState != VaultSessionState.Unlocked || _accountTransition || !settings.IsValid)
            return;
        var saved = VaultTimeoutStore.TrySave(_timeoutAccountKey, settings);
        _vaultTimeout = settings;
        RecordVaultActivity();
        UpdateVaultTimeoutMonitor(_observedVaultState);
        if (!saved) page.ReportTimeoutSaveFailure();
        else page.ReportTimeoutSaved();
    }

    private async Task CheckVaultTimeoutAsync()
    {
        if (_closed || _accountTransition || _signInDialogOpen || !CanApplyAutomaticTimeout(_observedVaultState)) return;

        var foreground = _windowsVaultEvents?.IsForeground() == true;
        var now = Environment.TickCount64;
        long? idle = null;
        if (WindowsVaultEvents.TryGetLastInput(out var input)) idle = WindowsVaultEvents.MillisecondsSinceInput(input);
        if (EffectiveTimeoutMode == VaultTimeoutMode.Inactivity)
        {
            var duration = (long)_vaultTimeout.Minutes * 60 * 1000;
            // A modal dialog can receive input outside the primary XAML visual tree.
            if (foreground && idle.HasValue && input != _lastWindowsInput)
            {
                _lastWindowsInput = input;
                var observed = now - WindowsVaultEvents.MillisecondsSinceInput(input);
                if (observed > _lastVaultActivity && observed - _lastVaultActivity < duration)
                    _lastVaultActivity = observed;
            }
        }
        if (!_vaultTimeout.IsExpired(EffectiveTimeoutMode, _observedVaultState, foreground, now - _lastVaultActivity, idle)) return;

        await ApplyAutomaticTimeoutAsync();
    }

    private void DiscardPendingWindowsTimeout()
    {
        _pendingWindowsTimeout = PendingWindowsTimeout.None;
        _windowsTimeoutGeneration++;
    }

    private void OnWindowsVaultEvent(VaultTimeoutMode mode)
    {
        if (_closed || _timeoutAccountKey is null) return;
        _pendingWindowsTimeout |= mode == VaultTimeoutMode.SystemLock
            ? PendingWindowsTimeout.Lock : PendingWindowsTimeout.Sleep;
        _ = ApplyPendingWindowsTimeoutAsync();
    }

    private async Task<bool> ApplyPendingWindowsTimeoutAsync()
    {
        if (_closed || _accountTransition || _signInDialogOpen || _pendingWindowsTimeout == PendingWindowsTimeout.None)
            return false;
        var pending = _pendingWindowsTimeout;
        _pendingWindowsTimeout = PendingWindowsTimeout.None;
        var expected = EffectiveTimeoutMode switch
        {
            VaultTimeoutMode.SystemLock => PendingWindowsTimeout.Lock,
            VaultTimeoutMode.SystemSleep => PendingWindowsTimeout.Sleep,
            _ => PendingWindowsTimeout.None,
        };
        if ((pending & expected) == PendingWindowsTimeout.None || !CanApplyAutomaticTimeout(_observedVaultState))
            return false;
        await ApplyAutomaticTimeoutAsync();
        return true;
    }

    private bool CanApplyAutomaticTimeout(VaultSessionState state) => _vaultTimeout.CanApply(state);

    private async Task ApplyAutomaticTimeoutAsync()
    {
        try
        {
            await ChangeSessionAsync(_vaultTimeout.Action == VaultTimeoutAction.LogOut ? RustCore.Logout : LockVault);
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { }
    }
}
