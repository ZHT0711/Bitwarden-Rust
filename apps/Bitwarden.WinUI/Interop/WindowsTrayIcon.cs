using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;

namespace Bitwarden_WinUI.Interop;

internal sealed class WindowsTrayIcon : IDisposable
{
    private const uint Add = 0, Delete = 2, SetVersion = 4, Version4 = 4;
    private const uint CallbackMessage = 0x8000 + 42;
    private const uint ContextMenu = 0x007B, Select = 0x0400, KeySelect = 0x0401;
    private const uint TrayIconId = 1, OpenCommand = 1, ExitCommand = 2;
    private const uint MenuFlags = 0x0100 | 0x0080 | 0x0002; // TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON
    private const nuint SubclassId = 2; // WindowsVaultEvents uses 1.

    private readonly nint _window;
    private readonly DispatcherQueue _dispatcher;
    private readonly Action _open, _exit, _unavailable;
    private readonly SubclassProc _callback;
    private readonly uint _taskbarCreated;
    private readonly nint _icon;
    private bool _attached, _registered, _disposed;

    internal WindowsTrayIcon(nint window, DispatcherQueue dispatcher, Action open, Action exit, Action unavailable)
    {
        _window = window;
        _dispatcher = dispatcher;
        _open = open;
        _exit = exit;
        _unavailable = unavailable;
        _callback = WindowProc;
        _taskbarCreated = RegisterWindowMessage("TaskbarCreated");
        _icon = LoadImage(0, Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico"), 1, 0, 0, 0x0010 | 0x0040);
        if (_icon != 0 && _taskbarCreated != 0)
            _attached = SetWindowSubclass(window, _callback, SubclassId, 0);
    }

    internal bool TryRegister()
    {
        if (_registered) return true;
        if (!_attached) return false;
        var data = IconData();
        if (!ShellNotifyIcon(Add, ref data)) return false;
        data.Version = Version4;
        if (!ShellNotifyIcon(SetVersion, ref data))
        {
            ShellNotifyIcon(Delete, ref data);
            return false;
        }
        _registered = true;
        return true;
    }

    private NotifyIconData IconData() => new()
    {
        Size = (uint)Marshal.SizeOf<NotifyIconData>(), Window = _window, Id = TrayIconId,
        Flags = 0x1 | 0x2 | 0x4 | 0x80, // NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP
        Callback = CallbackMessage, Icon = _icon, ToolTip = "Bitwarden Rust",
    };

    private nint WindowProc(nint window, uint message, nuint parameter, nint data, nuint id, nuint reference)
    {
        if (message == CallbackMessage && _registered && ((uint)data >> 16) == TrayIconId)
        {
            switch ((uint)data & 0xFFFF)
            {
                case Select or KeySelect:
                    _dispatcher.TryEnqueue(() => { if (!_disposed) _open(); });
                    break;
                case ContextMenu:
                    var x = unchecked((short)parameter);
                    var y = unchecked((short)(parameter >> 16));
                    _dispatcher.TryEnqueue(() => ShowMenu(x, y));
                    break;
            }
            return 0;
        }
        if (message == _taskbarCreated && _registered)
        {
            _registered = false;
            _dispatcher.TryEnqueue(() => { if (!_disposed && !TryRegister()) _unavailable(); });
        }
        return DefSubclassProc(window, message, parameter, data);
    }

    private void ShowMenu(int x, int y)
    {
        if (!_registered || _disposed) return;
        var menu = CreatePopupMenu();
        if (menu == 0) return;
        uint selected = 0;
        try
        {
            if (!AppendMenu(menu, 0, OpenCommand, "打开 Bitwarden Rust") ||
                !AppendMenu(menu, 0, ExitCommand, "退出 Bitwarden Rust")) return;
            SetForegroundWindow(_window);
            selected = TrackPopupMenuEx(menu, MenuFlags, x, y, _window, 0);
            // Required by the shell so the next tray popup dismisses correctly.
            PostMessage(_window, 0, 0, 0);
        }
        finally { DestroyMenu(menu); }
        if (selected == OpenCommand) _open();
        else if (selected == ExitCommand) _exit();
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (_registered)
        {
            var data = IconData();
            ShellNotifyIcon(Delete, ref data);
        }
        if (_attached) RemoveWindowSubclass(_window, _callback, SubclassId);
        if (_icon != 0) DestroyIcon(_icon);
        _registered = _attached = false;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData
    {
        internal uint Size;
        internal nint Window;
        internal uint Id, Flags, Callback;
        internal nint Icon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] internal string ToolTip;
        internal uint State, StateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] internal string Info;
        internal uint Version;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] internal string InfoTitle;
        internal uint InfoFlags;
        internal Guid Guid;
        internal nint BalloonIcon;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate nint SubclassProc(nint window, uint message, nuint parameter, nint data, nuint id, nuint reference);

    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconW", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessage(string name);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern nint LoadImage(nint instance, string name, uint type, int width, int height, uint flags);
    [DllImport("user32.dll")]
    private static extern bool DestroyIcon(nint icon);
    [DllImport("comctl32.dll")]
    private static extern bool SetWindowSubclass(nint window, SubclassProc callback, nuint id, nuint reference);
    [DllImport("comctl32.dll")]
    private static extern bool RemoveWindowSubclass(nint window, SubclassProc callback, nuint id);
    [DllImport("comctl32.dll")]
    private static extern nint DefSubclassProc(nint window, uint message, nuint parameter, nint data);
    [DllImport("user32.dll")]
    private static extern nint CreatePopupMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool AppendMenu(nint menu, uint flags, nuint item, string label);
    [DllImport("user32.dll")]
    private static extern bool DestroyMenu(nint menu);
    [DllImport("user32.dll")]
    private static extern uint TrackPopupMenuEx(nint menu, uint flags, int x, int y, nint window, nint options);
    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(nint window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool PostMessage(nint window, uint message, nuint parameter, nint data);
}
