using System.Runtime.InteropServices;

namespace Bytewarden.WinUI.Interop;

internal sealed class WindowsVaultEvents : IDisposable
{
    private const uint WmWtsSessionChange = 0x02B1;
    private const uint WmPowerBroadcast = 0x0218;
    private const nuint WtsSessionLock = 0x07;
    private const nuint PbtApmSuspend = 0x04;
    private const nuint PbtApmResumeAutomatic = 0x12;
    private const uint GaRootOwner = 3;
    private const uint NotifyForThisSession = 0;
    private const uint DeviceNotifyWindowHandle = 0;
    private const nuint SubclassId = 1;
    private readonly nint _window;
    private readonly Action<VaultTimeoutMode> _onEvent;
    private readonly SubclassProc _callback;
    private readonly nint _powerRegistration;
    private bool _subclassed;

    internal WindowsVaultEvents(nint window, Action<VaultTimeoutMode> onEvent)
    {
        _window = window;
        _onEvent = onEvent;
        _callback = WindowProc;
        _subclassed = SetWindowSubclass(window, _callback, SubclassId, 0);
        if (_subclassed)
        {
            CanMonitorLock = WTSRegisterSessionNotification(window, NotifyForThisSession);
            _powerRegistration = RegisterSuspendResumeNotification(window, DeviceNotifyWindowHandle);
        }
    }

    internal bool CanMonitorLock { get; }
    internal bool CanMonitorSleep => _powerRegistration != 0;

    internal bool IsForeground() => !IsIconic(_window) && GetAncestor(GetForegroundWindow(), GaRootOwner) == _window;

    internal static bool TryGetLastInput(out uint timestamp)
    {
        var info = new LastInputInfo { Size = (uint)Marshal.SizeOf<LastInputInfo>() };
        var success = GetLastInputInfo(ref info);
        timestamp = info.Time;
        return success;
    }

    internal static uint MillisecondsSinceInput(uint timestamp) => unchecked(GetTickCount() - timestamp);

    private nint WindowProc(nint window, uint message, nuint parameter, nint data, nuint subclassId, nuint reference)
    {
        if (message == WmWtsSessionChange && parameter == WtsSessionLock)
            _onEvent(VaultTimeoutMode.SystemLock);
        else if (message == WmPowerBroadcast && parameter is PbtApmSuspend or PbtApmResumeAutomatic)
            _onEvent(VaultTimeoutMode.SystemSleep);
        return DefSubclassProc(window, message, parameter, data);
    }

    public void Dispose()
    {
        if (!_subclassed) return;
        if (CanMonitorLock) WTSUnRegisterSessionNotification(_window);
        if (_powerRegistration != 0) UnregisterSuspendResumeNotification(_powerRegistration);
        RemoveWindowSubclass(_window, _callback, SubclassId);
        _subclassed = false;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct LastInputInfo
    {
        internal uint Size;
        internal uint Time;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate nint SubclassProc(nint window, uint message, nuint parameter, nint data, nuint id, nuint reference);

    [DllImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowSubclass(nint window, SubclassProc proc, nuint id, nuint reference);

    [DllImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RemoveWindowSubclass(nint window, SubclassProc proc, nuint id);

    [DllImport("comctl32.dll")]
    private static extern nint DefSubclassProc(nint window, uint message, nuint parameter, nint data);

    [DllImport("wtsapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool WTSRegisterSessionNotification(nint window, uint flags);

    [DllImport("wtsapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool WTSUnRegisterSessionNotification(nint window);

    [DllImport("user32.dll")]
    private static extern nint RegisterSuspendResumeNotification(nint recipient, uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnregisterSuspendResumeNotification(nint handle);

    [DllImport("user32.dll")]
    private static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsIconic(nint window);

    [DllImport("user32.dll")]
    private static extern nint GetAncestor(nint window, uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetLastInputInfo(ref LastInputInfo info);

    [DllImport("kernel32.dll")]
    private static extern uint GetTickCount();
}
