# Dot-sourced by verify-vault-list-ui.ps1; only public synthetic data and the isolated preview.
function InvokeClipboardStressChecks {
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;
public sealed class ClipboardStressHold : IDisposable {
    readonly ManualResetEventSlim ready = new(false), release = new(false);
    readonly Thread thread;
    Exception failure;
    public ClipboardStressHold() {
        thread = new Thread(() => {
            bool opened = false;
            try {
                var deadline = DateTime.UtcNow.AddSeconds(2);
                while (!(opened = ClipboardStressNative.OpenClipboard(IntPtr.Zero))) {
                    if (DateTime.UtcNow >= deadline) throw new Exception("Cannot hold synthetic clipboard.");
                    Thread.Sleep(10);
                }
                ready.Set();
                if (!release.Wait(12000)) throw new Exception("Synthetic clipboard hold safety deadline expired.");
            } catch (Exception error) { failure = error; ready.Set(); }
            finally { if (opened) ClipboardStressNative.CloseClipboard(); }
        });
        thread.IsBackground = true;
        thread.Start();
        if (!ready.Wait(3000) || failure != null) {
            release.Set(); thread.Join(3000);
            throw new Exception("Synthetic clipboard hold did not become ready.", failure);
        }
    }
    public void Dispose() {
        release.Set();
        if (!thread.Join(3000)) throw new Exception("Synthetic clipboard hold did not finish.");
        ready.Dispose(); release.Dispose();
        if (failure != null) throw failure;
    }
}
public static class ClipboardStressNative {
    [DllImport("user32.dll")] public static extern bool OpenClipboard(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool CloseClipboard();
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint process);
    delegate bool EnumWindow(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumThreadWindows(uint thread, EnumWindow callback, IntPtr data);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, System.Text.StringBuilder name, int size);
    [DllImport("user32.dll")] static extern IntPtr GetDesktopWindow();
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint message, UIntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern uint RegisterClipboardFormat(string name);
    [DllImport("user32.dll")] static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll")] static extern bool IsClipboardFormatAvailable(uint format);
    [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr handle);
    [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr handle);
    public static void CloseWindow(IntPtr hwnd) {
        if (!PostMessage(hwnd, 0x10, UIntPtr.Zero, IntPtr.Zero)) throw new Exception("Cannot request synthetic window close.");
    }
    // Exercise the application's real registered tray callback/menu, without a test-only app hook.
    public static void TrayRestore(IntPtr hwnd) {
        if (!PostMessage(hwnd, 0x802a, UIntPtr.Zero, new IntPtr((1 << 16) | 0x400))) throw new Exception("Cannot send tray restore.");
    }
    public static void TrayMenu(IntPtr hwnd) {
        if (!PostMessage(hwnd, 0x802a, new UIntPtr((300u << 16) | 300u), new IntPtr((1 << 16) | 0x7b))) throw new Exception("Cannot show tray menu.");
    }
    public static IntPtr TrayMenuWindow(IntPtr owner) {
        uint process;
        var thread = GetWindowThreadProcessId(owner, out process);
        var deadline = DateTime.UtcNow.AddSeconds(2);
        do {
            IntPtr found = IntPtr.Zero;
            EnumThreadWindows(thread, (hwnd, data) => {
                var name = new System.Text.StringBuilder(64);
                GetClassName(hwnd, name, name.Capacity);
                if (name.ToString() == "#32768" && IsWindowVisible(hwnd)) found = hwnd;
                return true;
            }, IntPtr.Zero);
            if (found != IntPtr.Zero) return found;
            Thread.Sleep(20);
        } while (DateTime.UtcNow < deadline);
        throw new Exception("The application's native tray menu did not appear.");
    }
    static void OpenRead() {
        var deadline = DateTime.UtcNow.AddSeconds(2);
        while (!OpenClipboard(GetDesktopWindow())) {
            if (DateTime.UtcNow >= deadline) throw new Exception("Cannot inspect synthetic clipboard formats.");
            Thread.Sleep(10);
        }
    }
    public static bool HasOwnerFormat() {
        OpenRead();
        try { return IsClipboardFormatAvailable(RegisterClipboardFormat("BitwardenRust.ClipboardOwner")); }
        finally { CloseClipboard(); }
    }
    public static int ReadFlag(string name) {
        OpenRead();
        try {
            var handle = GetClipboardData(RegisterClipboardFormat(name));
            if (handle == IntPtr.Zero) return -1;
            var pointer = GlobalLock(handle);
            if (pointer == IntPtr.Zero) return -1;
            try { return Marshal.ReadInt32(pointer); }
            finally { GlobalUnlock(handle); }
        } finally { CloseClipboard(); }
    }
}
'@
    function WaitStressEmpty([string]$Name) {
        $deadline = [DateTime]::UtcNow.AddSeconds(8)
        do {
            if ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) { break }
            Start-Sleep -Milliseconds 200
        } while ([DateTime]::UtcNow -lt $deadline)
        Check ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) $Name
    }
    function RestartStressPreview([string]$Reason) {
        $launch = WinApp @('run', $layout, '--detach', '--devtools', 'off', '--json') | ConvertFrom-Json
        $script:previewId = [int]$launch.ProcessId
        WaitPreviewWindow
        [void](WaitUi '保险库已锁定')
        Resize 1000 1500
        Click 'LockButton'
        [void](WaitUi '仅离线解锁')
        SetValue '主密码' 'asdfasdfasdf'
        Click 'PrimaryButton'
        [void](WaitUi '保险库已加载')
        Click 'Synthetic login' 'ListItem'
        [void](WaitUi '修改时间')
        Check ((Tree) -match 'fixture-user') "synthetic cache reconnects after $Reason"
    }
    function SetStressTray([bool]$Enabled) {
        if ((Tree) -notmatch 'SettingsItem') { Click 'PART_PaneToggleButton' }
        Click 'SettingsItem'
        [void](WaitUi 'KeepInTrayToggle')
        $action = if ($Enabled) { 'toggle-on' } else { 'toggle-off' }
        [void](WinApp @('ui', 'invoke', 'KeepInTrayToggle', '-w', [string](Get-Process -Id $previewId).MainWindowHandle, '--action', $action))
        if ((Tree) -notmatch 'VaultNavigationItem') { Click 'PART_PaneToggleButton' }
        Click 'VaultNavigationItem'
        [void](WaitUi 'VaultList')
        Click 'Synthetic login' 'ListItem'
        [void](WaitUi '修改时间')
    }

    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'stress copy contains only the synthetic username'
    $history = [ClipboardStressNative]::ReadFlag('CanIncludeInClipboardHistory')
    $cloud = [ClipboardStressNative]::ReadFlag('CanUploadToCloudClipboard')
    [ordered]@{ historyFlag = $history; cloudFlag = $cloud; nativeOwnerFormat = [ClipboardStressNative]::HasOwnerFormat() } |
        ConvertTo-Json | Set-Content (Join-Path $run 'clipboard-formats.json')
    Check ($history -eq 0 -and $cloud -eq 0) 'actual clipboard package explicitly excludes history and cloud upload'
    Check ([ClipboardStressNative]::HasOwnerFormat()) 'WinRT private owner marker is present as a native clipboard format'
    Start-Sleep -Milliseconds 9300
    [SyntheticListWindow]::HoldClipboard(6500)
    WaitStressEmpty 'owned clipboard clears after more than five seconds of native contention at expiry'

    # A fresh copy before the first copy's deadline must not inherit its pending timer.
    Click '复制用户名' 'Button'
    Start-Sleep -Milliseconds 8500
    Click '复制用户名' 'Button'
    Start-Sleep -Milliseconds 2500
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'new app copy survives the previous copy deadline'
    WaitStressEmpty 'new app copy clears only at its own configured deadline'

    Click '复制用户名' 'Button'
    Start-Sleep -Milliseconds 9300
    [SyntheticListWindow]::HoldClipboard(6500)
    Set-Clipboard -Value 'Synthetic writer during pending retry'
    Start-Sleep -Milliseconds 1800
    Check ((Get-Clipboard -Raw) -eq 'Synthetic writer during pending retry') 'long-contention retry preserves a subsequent external writer'

    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'lock contention starts with an owned synthetic copy'
    $hold = [ClipboardStressHold]::new()
    try {
        Keys 'ctrl+l'
        $tree = WaitUi '保险库已锁定'
        Check ($tree -notmatch 'fixture-user|ListItem.*"Synthetic') 'vault locking clears sensitive UI even while clipboard cleanup is busy'
        Start-Sleep -Milliseconds 1300
    } finally { $hold.Dispose() }
    WaitStressEmpty 'vault lock retries owned clipboard cleanup after native contention releases'
    Click 'LockButton'
    SetValue '主密码' 'asdfasdfasdf'
    Click 'PrimaryButton'
    [void](WaitUi '保险库已加载')
    Click 'Synthetic login' 'ListItem'
    [void](WaitUi '修改时间')

    SetStressTray $false
    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'normal exit starts with an owned synthetic copy'
    $app = Get-Process -Id $previewId
    $hwnd = $app.MainWindowHandle
    $hold = [ClipboardStressHold]::new()
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        [ClipboardStressNative]::CloseWindow($hwnd)
        Start-Sleep -Milliseconds 250
        [ClipboardStressNative]::CloseWindow($hwnd)
        $tree = Tree
        $tree | Set-Content (Join-Path $run 'clipboard-closing-ui.txt')
        Check (-not $app.HasExited -and $tree -notmatch 'fixture-user|ListItem.*"Synthetic') 'duplicate close requests wait for cleanup after immediately revoking sensitive UI'
        [void](WinApp @('ui', 'screenshot', '-w', [string]$hwnd, '-o', (Join-Path $run 'clipboard-closing.png')))
        if ($watch.ElapsedMilliseconds -lt 1800) { Start-Sleep -Milliseconds (1800 - [int]$watch.ElapsedMilliseconds) }
    } finally { $hold.Dispose() }
    Check ($app.WaitForExit(10000) -and [string]::IsNullOrEmpty((Get-Clipboard -Raw))) 'normal exit waits for a busy owned clipboard to clear before terminating'
    $normalExitMs = $watch.ElapsedMilliseconds
    RestartStressPreview 'normal contention exit'

    # Windows cannot clear a clipboard which another process keeps locked forever.
    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'bounded exit starts with an owned synthetic copy'
    $app = Get-Process -Id $previewId
    $hwnd = $app.MainWindowHandle
    $hold = [ClipboardStressHold]::new()
    $watch.Restart()
    try {
        [ClipboardStressNative]::CloseWindow($hwnd)
        Start-Sleep -Milliseconds 1400
        [ClipboardStressNative]::CloseWindow($hwnd)
        Check (-not $app.HasExited) 'exit does not abandon clipboard cleanup immediately under prolonged contention'
        Check ($app.WaitForExit(6500) -and $watch.ElapsedMilliseconds -ge 4500 -and $watch.ElapsedMilliseconds -lt 7900) `
            'exit remains bounded when an external process keeps the clipboard locked past the five-second budget'
        $boundedExitMs = $watch.ElapsedMilliseconds
    } finally { $hold.Dispose() }
    $retainedAfterDeadline = (Get-Clipboard -Raw) -eq 'fixture-user'
    Set-Clipboard -Value 'Synthetic writer after bounded exit'
    Start-Sleep -Milliseconds 1500
    Check ((Get-Clipboard -Raw) -eq 'Synthetic writer after bounded exit') 'terminated cleanup never erases a writer after the bounded exit'
    RestartStressPreview 'bounded contention exit'

    SetStressTray $true
    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'tray test starts with an owned synthetic copy'
    $app = Get-Process -Id $previewId
    $hwnd = $app.MainWindowHandle
    [ClipboardStressNative]::CloseWindow($hwnd)
    Start-Sleep -Milliseconds 500
    Check (-not $app.HasExited -and -not [ClipboardStressNative]::IsWindowVisible($hwnd) -and
        (Get-Clipboard -Raw) -eq 'fixture-user') 'close-to-tray keeps the account alive and does not run terminal cleanup'
    [ClipboardStressNative]::TrayRestore($hwnd)
    WaitPreviewWindow
    [void](WaitUi '修改时间')
    Check ([ClipboardStressNative]::IsWindowVisible($hwnd)) 'registered tray restore callback reopens the same unlocked window'
    # Prepare a native clipboard lock, then choose the actual native tray menu exit command.
    $hold = [ClipboardStressHold]::new()
    try {
        [ClipboardStressNative]::TrayMenu($hwnd)
        $menu = [ClipboardStressNative]::TrayMenuWindow($hwnd)
        WinApp @('ui', 'inspect', '-w', [string]$menu, '--depth', '5') | Set-Content (Join-Path $run 'clipboard-tray-menu.txt')
        [void](WinApp @('ui', 'invoke', '退出 Bitwarden Rust', '-w', [string]$menu, '--type', 'MenuItem'))
        Start-Sleep -Milliseconds 600
        Check (-not $app.HasExited -and (Tree) -notmatch 'fixture-user|ListItem.*"Synthetic') `
            'native tray exit revokes sensitive UI and waits while owned clipboard access is busy'
    } finally { $hold.Dispose() }
    Check ($app.WaitForExit(10000) -and [string]::IsNullOrEmpty((Get-Clipboard -Raw))) 'native tray menu exit clears the owned clipboard before terminating'
    RestartStressPreview 'native tray exit'
    SetStressTray $false
    [ordered]@{ normalExitMs = $normalExitMs; boundedExitMs = $boundedExitMs; ownedSyntheticTextRetainedAfterBudget = $retainedAfterDeadline } |
        ConvertTo-Json | Set-Content (Join-Path $run 'clipboard-exit-observations.json')
}
