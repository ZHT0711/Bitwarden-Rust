namespace Bitwarden_WinUI.Interop;

internal enum VaultTimeoutMode
{
    AppRestart,
    Inactivity,
    SystemIdle,
    SystemLock,
    SystemSleep,
    Never,
}

internal enum VaultTimeoutAction
{
    Lock,
    LogOut,
}

internal readonly record struct VaultTimeoutSettings(VaultTimeoutMode Mode, int Minutes, VaultTimeoutAction Action)
{
    internal const int MaximumMinutes = int.MaxValue;
    internal static VaultTimeoutSettings Default => new(VaultTimeoutMode.AppRestart, 15, VaultTimeoutAction.Lock);
    internal bool IsValid => Enum.IsDefined(Mode) && Enum.IsDefined(Action) && Minutes >= 1;
    internal const long SystemIdleMilliseconds = 5 * 60 * 1000;

    internal static string AccountKey(uint serverMode, string address, string email)
    {
        return "VaultTimeout." + AccountSettingsScope.Create(serverMode, address, email);
    }

    internal VaultTimeoutMode EffectiveMode(bool canMonitorLock, bool canMonitorSleep) => Mode switch
    {
        VaultTimeoutMode.SystemLock when !canMonitorLock => canMonitorSleep ? VaultTimeoutMode.SystemSleep : VaultTimeoutMode.AppRestart,
        VaultTimeoutMode.SystemSleep when !canMonitorSleep => VaultTimeoutMode.AppRestart,
        _ => Mode,
    };

    internal bool CanApply(VaultSessionState state) => state == VaultSessionState.Unlocked ||
        (state == VaultSessionState.Locked && Action == VaultTimeoutAction.LogOut);

    internal bool IsExpired(VaultTimeoutMode effectiveMode, VaultSessionState state, bool isForeground,
        long elapsedActivityMilliseconds, long? systemIdleMilliseconds)
    {
        if (!IsValid || !CanApply(state)) return false;
        return effectiveMode switch {
            VaultTimeoutMode.Inactivity => state == VaultSessionState.Unlocked && !isForeground &&
                elapsedActivityMilliseconds >= (long)Minutes * 60 * 1000,
            VaultTimeoutMode.SystemIdle => systemIdleMilliseconds >= SystemIdleMilliseconds,
            _ => false,
        };
    }
}
