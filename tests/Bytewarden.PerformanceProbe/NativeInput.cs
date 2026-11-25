using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

internal static class NativeInput
{
    [StructLayout(LayoutKind.Sequential)] private struct Input { public uint Type; public InputUnion Data; }
    [StructLayout(LayoutKind.Explicit)] private struct InputUnion
    {
        [FieldOffset(0)] public Keyboard Keyboard;
        [FieldOffset(0)] public Mouse Mouse;
    }
    [StructLayout(LayoutKind.Sequential)] private struct Keyboard
    {
        public ushort VirtualKey, ScanCode;
        public uint Flags, Time;
        public IntPtr ExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)] private struct Mouse
    {
        public int X, Y;
        public uint Data, Flags, Time;
        public IntPtr ExtraInfo;
    }
    [DllImport("user32.dll", SetLastError = true)] private static extern uint SendInput(uint count, Input[] inputs, int size);
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] private static extern int GetPackageFullName(IntPtr process, ref uint length, StringBuilder? name);

    internal static void RequireSyntheticPackage(Process process, IntPtr hwnd)
    {
        GetWindowThreadProcessId(hwnd, out var owner);
        if (owner != process.Id) throw new InvalidOperationException("Target HWND belongs to another process.");
        uint length = 0;
        if (GetPackageFullName(process.Handle, ref length, null) != 122 || length is 0 or > 1000)
            throw new InvalidOperationException("Target has no valid synthetic package identity.");
        var name = new StringBuilder((int)length);
        if (GetPackageFullName(process.Handle, ref length, name) != 0 ||
            !name.ToString().StartsWith("ZHT0711.BytewardenSyntheticListTest_", StringComparison.Ordinal))
            throw new InvalidOperationException("Input/resource probe refuses real or non-test packages.");
    }

    internal static void RequireForeground(int pid)
    {
        GetWindowThreadProcessId(GetForegroundWindow(), out var foreground);
        if (foreground != pid) throw new InvalidOperationException("Operator changed foreground window; stop rather than inject input into another app.");
    }

    internal static void SendCharacter(char character)
    {
        Input[] inputs = [
            new() { Type = 1, Data = new InputUnion { Keyboard = new Keyboard { ScanCode = character, Flags = 4 } } },
            new() { Type = 1, Data = new InputUnion { Keyboard = new Keyboard { ScanCode = character, Flags = 6 } } }
        ];
        if (SendInput(2, inputs, Marshal.SizeOf<Input>()) != 2)
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Synthetic Unicode input was not injected completely.");
    }
}
