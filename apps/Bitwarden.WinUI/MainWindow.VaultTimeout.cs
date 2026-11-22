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

    private sealed class VaultActivity(VaultTimeoutSettings settings)
    {
        internal VaultTimeoutSettings Settings { get; set; } = settings;
        internal long LastActivity { get; set; } = Environment.TickCount64;
        internal uint LastWindowsInput { get; set; }
    }

    private readonly Dictionary<string, VaultActivity> _accountActivity = new();
    private WindowsVaultEvents? _windowsVaultEvents;
    private DispatcherQueueTimer? _vaultTimeoutTimer;
    private PendingWindowsTimeout _pendingWindowsTimeout;

    private void InitializeVaultTimeout()
    {
        var dispatcher = DispatcherQueue;
        _windowsVaultEvents = new(WinRT.Interop.WindowNative.GetWindowHandle(this), mode =>
            dispatcher.TryEnqueue(DispatcherQueuePriority.High, () => OnWindowsVaultEvent(mode)));
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
            if (args.WindowActivationState != WindowActivationState.Deactivated) _ = CheckVaultTimeoutAsync();
        };
    }

    private VaultAccount[] ApplyRestartTimeouts(VaultAccount[] accounts)
    {
        var targets = new List<VaultAccountTimeout>();
        foreach (var account in accounts)
        {
            var settings = VaultTimeoutStore.Load(TimeoutKey(account));
            if (settings.Action == VaultTimeoutAction.LogOut && settings.EffectiveMode(
                _windowsVaultEvents?.CanMonitorLock == true, _windowsVaultEvents?.CanMonitorSleep == true) == VaultTimeoutMode.AppRestart)
                targets.Add(new(account.Id, LogOut: true));
        }
        return targets.Count == 0 ? accounts : RustCore.ApplyAccountTimeouts(targets.ToArray(), interrupt: true);
    }

    private void RetainAccountTimeouts(VaultAccount[] accounts)
    {
        foreach (var id in _accountActivity.Keys.Where(id => accounts.All(account => account.Id != id)).ToArray())
            _accountActivity.Remove(id);
        foreach (var account in accounts)
        {
            if (!_accountActivity.TryGetValue(account.Id, out var activity))
            {
                activity = new(VaultTimeoutStore.Load(TimeoutKey(account)));
                _accountActivity.Add(account.Id, activity);
            }
            if (account.State == VaultSessionState.Unlocked &&
                _accounts.Any(previous => previous.Id == account.Id && previous.State == VaultSessionState.Locked))
                RecordActivity(activity);
        }
    }

    private static string TimeoutKey(VaultAccount account) =>
        VaultTimeoutSettings.AccountKey(account.ServerMode, account.ServerUrl, account.Email);

    private void RecordVaultActivity()
    {
        if (ActiveAccount is { State: VaultSessionState.Unlocked } account &&
            _accountActivity.TryGetValue(account.Id, out var activity)) RecordActivity(activity);
    }

    private static void RecordActivity(VaultActivity activity)
    {
        activity.LastActivity = Environment.TickCount64;
        if (WindowsVaultEvents.TryGetLastInput(out var input)) activity.LastWindowsInput = input;
    }

    private VaultTimeoutMode EffectiveTimeoutMode(VaultActivity activity) => activity.Settings.EffectiveMode(
        _windowsVaultEvents?.CanMonitorLock == true, _windowsVaultEvents?.CanMonitorSleep == true);

    private bool TimeoutAvailable => !_closed && !_accountTransition && !_signInDialogOpen;

    private void UpdateVaultTimeoutMonitor()
    {
        if (NavFrame.Content is SettingsPage page) UpdateTimeoutControls(page);
        if (_vaultTimeoutTimer is null) return;
        _vaultTimeoutTimer.Stop();
        if (!TimeoutAvailable) return;
        var modes = _accounts.Select(account => (account.State, Activity: _accountActivity[account.Id]))
            .Where(entry => entry.Activity.Settings.CanApply(entry.State))
            .Select(entry => EffectiveTimeoutMode(entry.Activity)).ToArray();
        if (modes.Contains(VaultTimeoutMode.Inactivity) || modes.Contains(VaultTimeoutMode.SystemIdle))
        {
            _vaultTimeoutTimer.Interval = TimeSpan.FromSeconds(modes.Contains(VaultTimeoutMode.Inactivity) ? 10 : 30);
            _vaultTimeoutTimer.Start();
        }
    }

    private void UpdateTimeoutControls(SettingsPage page)
    {
        var account = ActiveAccount;
        var settings = account is null ? VaultTimeoutSettings.Default : _accountActivity[account.Id].Settings;
        page.ShowVaultTimeout(settings, account?.State == VaultSessionState.Unlocked && TimeoutAvailable,
            _windowsVaultEvents?.CanMonitorLock == true, _windowsVaultEvents?.CanMonitorSleep == true);
    }

    private void SaveVaultTimeout(SettingsPage page, VaultTimeoutSettings settings)
    {
        if (ActiveAccount is not { State: VaultSessionState.Unlocked } account || !TimeoutAvailable || !settings.IsValid)
            return;
        var saved = VaultTimeoutStore.TrySave(TimeoutKey(account), settings);
        _accountActivity[account.Id].Settings = settings;
        RecordVaultActivity();
        UpdateVaultTimeoutMonitor();
        if (!saved) page.ReportTimeoutSaveFailure();
        else page.ReportTimeoutSaved();
    }

    private async Task CheckVaultTimeoutAsync()
    {
        if (!TimeoutAvailable || await ApplyPendingWindowsTimeoutAsync() || !TimeoutAvailable) return;
        var foreground = _windowsVaultEvents?.IsForeground() == true;
        var now = Environment.TickCount64;
        var hasInput = WindowsVaultEvents.TryGetLastInput(out var input);
        long? idle = hasInput ? WindowsVaultEvents.MillisecondsSinceInput(input) : null;
        var targets = new List<VaultAccountTimeout>();
        foreach (var account in _accounts)
        {
            var activity = _accountActivity[account.Id];
            var mode = EffectiveTimeoutMode(activity);
            var accountForeground = account.Active && foreground;
            if (mode == VaultTimeoutMode.Inactivity && accountForeground && idle.HasValue && input != activity.LastWindowsInput)
            {
                activity.LastWindowsInput = input;
                var observed = now - idle.Value;
                var duration = (long)activity.Settings.Minutes * 60 * 1000;
                if (observed > activity.LastActivity && observed - activity.LastActivity < duration)
                    activity.LastActivity = observed;
            }
            if (activity.Settings.IsExpired(mode, account.State, accountForeground, now - activity.LastActivity, idle))
                targets.Add(new(account.Id, activity.Settings.Action == VaultTimeoutAction.LogOut));
        }
        await ApplyAccountTimeoutsAsync(targets.ToArray(), systemEvent: false);
    }

    private void OnWindowsVaultEvent(VaultTimeoutMode mode)
    {
        if (_closed || (_accounts.Length == 0 && !_signInDialogOpen)) return;
        // A Windows lock/suspend must not leave an in-flight Hello prompt able to unlock afterward.
        _unlockRequest?.Cancel();
        _pendingWindowsTimeout |= mode == VaultTimeoutMode.SystemLock
            ? PendingWindowsTimeout.Lock : PendingWindowsTimeout.Sleep;
        _ = ApplyPendingWindowsTimeoutAsync();
    }

    private async Task<bool> ApplyPendingWindowsTimeoutAsync()
    {
        if (!TimeoutAvailable || _pendingWindowsTimeout == PendingWindowsTimeout.None) return false;
        var pending = _pendingWindowsTimeout;
        _pendingWindowsTimeout = PendingWindowsTimeout.None;
        var targets = new List<VaultAccountTimeout>();
        foreach (var account in _accounts)
        {
            var activity = _accountActivity[account.Id];
            var expected = EffectiveTimeoutMode(activity) switch
            {
                VaultTimeoutMode.SystemLock => PendingWindowsTimeout.Lock,
                VaultTimeoutMode.SystemSleep => PendingWindowsTimeout.Sleep,
                _ => PendingWindowsTimeout.None,
            };
            if ((pending & expected) != 0 && activity.Settings.CanApply(account.State))
                targets.Add(new(account.Id, activity.Settings.Action == VaultTimeoutAction.LogOut));
        }
        return await ApplyAccountTimeoutsAsync(targets.ToArray(), systemEvent: true);
    }

    private async Task<bool> ApplyAccountTimeoutsAsync(VaultAccountTimeout[] actions, bool systemEvent)
    {
        if (actions.Length == 0) return false;
        var activeId = ActiveAccount?.Id;
        var affectsActive = actions.Any(action => action.Id == activeId);
        try
        {
            await ChangeAccountsAsync(() => RustCore.ApplyAccountTimeouts(actions, affectsActive || systemEvent), affectsActive);
            return affectsActive;
        }
        catch (VaultException error) when (error.Code == "busy") { return false; }
        catch (VaultException error) when (error.Code == "cacheRevocationFailed")
        {
            if (!_closed) WarnAboutAccountStorage("自动登出未完成", error.UserMessage);
            return affectsActive;
        }
        catch (Exception error) when (IsCoreUnavailable(error)) { return false; }
    }
}
