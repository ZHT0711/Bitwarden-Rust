param(
    [string]$Package = 'apps/Bitwarden.WinUI/AppPackages/Bitwarden.WinUI_0.2.0.0_x64_Test/Bitwarden.WinUI_0.2.0.0_x64.msix',
    [string]$Fixture = 'work/ci-fixture.json',
    [switch]$IncludeFolders,
    [switch]$IncludeClipboardStress,
    [switch]$CheckLinkedFields,
    [switch]$MeasureFolders,
    [ValidateRange(3, 20)][int]$FolderSamples = 5,
    [ValidateSet('baseline', 'optimized')][string]$FolderRunLabel = 'baseline'
)

# Uses only the SDK's public synthetic account and a loopback fixture server.
# It unpacks the actual MSIX, never the potentially stale bin/.../AppX layout.
$ErrorActionPreference = 'Stop'
if ($CheckLinkedFields -and ($IncludeFolders -or $IncludeClipboardStress -or $MeasureFolders)) {
    throw 'Linked-field mode is separate; do not combine it with folder or clipboard regression switches.'
}
if ($MeasureFolders -and ($IncludeFolders -or $IncludeClipboardStress)) {
    throw 'Folder pressure mode is separate; do not combine -MeasureFolders with regression switches.'
}
$root = Split-Path $PSScriptRoot -Parent
$winAppExe = (Get-Command winapp -CommandType Application -ErrorAction Stop).Source
$packagePath = (Resolve-Path (Join-Path $root $Package)).Path
$fixturePath = (Resolve-Path (Join-Path $root $Fixture)).Path
if ($CheckLinkedFields) {
    $linkedFixture = [IO.File]::ReadAllText($fixturePath) | ConvertFrom-Json
    if ($linkedFixture.linkedFieldFixture.synthetic -ne $true -or $linkedFixture.ciphers.Count -ne 6) {
        throw 'Expected the dedicated six-cipher public linked-field fixture.'
    }
}
$run = Join-Path $root ('work/list-acceptance-' + [Guid]::NewGuid().ToString('N'))
$layout = Join-Path $run 'app'
New-Item -ItemType Directory -Path $run | Out-Null
$identity = 'ZHT0711.BitwardenRustSyntheticListTest'
$server = $null
$previewId = $null
$baseUrl = $null
$registered = $false
$checks = 0

function WinApp([string[]]$CommandArgs) {
    # A successful command may warn about multiple HWNDs on stderr. That is not its value.
    $stderr = Join-Path $run 'winapp-stderr.log'
    $summary = ($CommandArgs | Select-Object -First 3) -join ' '
    "$(Get-Date -Format o) $summary" | Add-Content (Join-Path $run 'commands.log')
    $readOnly = $CommandArgs[0] -eq 'ui' -and $CommandArgs[1] -in @('inspect', 'get-value', 'get-property', 'get-focused')
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        $output = & $winAppExe @CommandArgs 2> $stderr | Out-String
        $status = $LASTEXITCODE
        if (-not $status) { return $output -replace '\x1b\[[0-9;]*m', '' }
        $errorText = [IO.File]::ReadAllText($stderr)
        if (-not $readOnly -or $attempt -eq 2 -or $errorText -notmatch 'no longer accessible') {
            throw "winapp $summary failed: $output $errorText"
        }
        # UIA reads are safe to repeat when a layout update invalidates its previous element.
        Start-Sleep -Milliseconds 200
    }
}
function Click([string]$Selector, [string]$Type = '') {
    $popupChoice = $Selector -in @('自托管 / 自定义服务器', '名称（Z → A）', '名称（A → Z）', '深色', '10 秒')
    $target = if ($popupChoice) { @('-a', [string]$previewId) } else { @('-w', [string](Get-Process -Id $previewId).MainWindowHandle) }
    $command = @('ui', 'invoke', $Selector) + $target
    if ($Type) { $command += @('--type', $Type) }
    [void](WinApp $command)
}
function SetValue([string]$Selector, [string]$Value) {
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        try {
            $hwnd = (Get-Process -Id $previewId).MainWindowHandle
            [void](WinApp @('ui', 'set-value', $Selector, $Value, '--type', 'Edit', '-w', [string]$hwnd))
            return
        } catch {
            if ($attempt -eq 2) { throw }
            # Setting the same text is idempotent; allow a just-navigated WinUI tree to settle.
            [void](Tree)
            Start-Sleep -Milliseconds 250
        }
    }
}
function WaitPreviewWindow {
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    do {
        if ((Get-Process -Id $previewId).MainWindowHandle -ne [IntPtr]::Zero) { return }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Synthetic preview did not create its main window.'
}
function Tree {
    $hwnd = (Get-Process -Id $previewId).MainWindowHandle
    WinApp @('ui', 'inspect', '-w', [string]$hwnd, '--depth', '20')
}
function WaitUi([string]$Pattern) {
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    do {
        $tree = Tree
        if ($tree -match $Pattern) { return $tree }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    $tree | Set-Content (Join-Path $run 'unexpected-ui.txt')
    throw "UI did not reach: $Pattern"
}
function WaitValue([string]$Selector, [string]$Expected) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        try {
            $hwnd = (Get-Process -Id $previewId).MainWindowHandle
            $actual = (WinApp @('ui', 'get-value', $Selector, '-w', [string]$hwnd)).Trim()
        } catch { $actual = '<not ready>' }
        if ($actual -eq $Expected) { return $actual }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    Tree | Set-Content (Join-Path $run 'unexpected-value-ui.txt')
    throw "Value did not settle for $Selector : expected [$Expected], actual [$actual]"
}
function Check([bool]$Condition, [string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Name)) { throw 'Every UI check must have a descriptive evidence label.' }
    if (-not $Condition) {
        try { Tree | Set-Content (Join-Path $run 'failed-check-ui.txt') } catch { }
        throw "FAILED: $Name"
    }
    $script:checks++
    "PASS: $Name" | Tee-Object -FilePath (Join-Path $run 'checks.log') -Append | Write-Host
}
function Keys([string]$Value) {
    $hwnd = [string](Get-Process -Id $previewId).MainWindowHandle
    # The CLI verifies both foreground and keyboard focus; never bypass its send-input guard.
    [void](WinApp @('ui', 'focus', '-w', $hwnd))
    [void](WinApp @('ui', 'send-keys', $Value, '-w', $hwnd, '--via', 'send-input'))
}
function ChooseSort([string]$Label) { Click 'VaultSortChoice'; Click $Label }
function Resize([int]$Width, [int]$Height) {
    $old = [SyntheticListWindow]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
    try {
        $hwnd = (Get-Process -Id $previewId).MainWindowHandle
        if (-not [SyntheticListWindow]::SetWindowPos($hwnd, [IntPtr]::Zero, 100, 100, $Width, $Height, 4)) {
            throw 'Cannot resize the synthetic preview window.'
        }
    } finally { [void][SyntheticListWindow]::SetThreadDpiAwarenessContext($old) }
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SyntheticListWindow {
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool OpenClipboard(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool CloseClipboard();
    public static void HoldClipboard(int milliseconds) {
        if (!OpenClipboard(IntPtr.Zero)) throw new InvalidOperationException("Cannot open the native clipboard.");
        try { System.Threading.Thread.Sleep(milliseconds); }
        finally { if (!CloseClipboard()) throw new InvalidOperationException("Cannot close the native clipboard."); }
    }
}
'@

try {
    [ordered]@{
        package = $packagePath
        packageSha256 = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash
        fixtureSha256 = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash
        includeFolders = [bool]$IncludeFolders
        includeClipboardStress = [bool]$IncludeClipboardStress
        checkLinkedFields = [bool]$CheckLinkedFields
        measureFolders = [bool]$MeasureFolders
        folderSamples = $FolderSamples
        folderRunLabel = $FolderRunLabel
        folderSourceHashes = [ordered]@{
            home = (Get-FileHash (Join-Path $root 'apps/Bitwarden.WinUI/Pages/HomePage.xaml.cs')).Hash
            navigation = (Get-FileHash (Join-Path $root 'apps/Bitwarden.WinUI/Pages/HomePage.Folders.cs')).Hash
            hierarchy = (Get-FileHash (Join-Path $root 'apps/Bitwarden.WinUI/Interop/FolderHierarchy.cs')).Hash
            homeXaml = (Get-FileHash (Join-Path $root 'apps/Bitwarden.WinUI/Pages/HomePage.xaml')).Hash
        }
    } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $run 'inputs.json')
    [void](WinApp @('tool', 'makeappx', 'unpack', '/p', $packagePath, '/d', $layout, '/o'))
    $manifest = Join-Path $layout 'AppxManifest.xml'
    [xml]$xml = [IO.File]::ReadAllText($manifest)
    $xml.Package.Identity.SetAttribute('Name', $identity)
    $xml.Package.Properties.DisplayName = 'Bitwarden Rust (Synthetic List Test)'
    $application = $xml.Package.Applications.Application
    $extensions = $application.SelectSingleNode("*[local-name()='Extensions']")
    if ($extensions) { [void]$application.RemoveChild($extensions) }
    $xml.Save($manifest)
    # Refuse to disturb a previous, still-running test or overwrite its data.
    if (Get-AppxPackage -Name $identity) { throw 'A synthetic list test package is already registered. Close and unregister that test first.' }
    $portFile = Join-Path $run 'fixture.port'
    $server = Start-Process -FilePath (Get-Command python).Source -ArgumentList @(
        '-u', 'tests/fixture_server.py', '--fixture', ('"' + $fixturePath + '"'), '--port-file', ('"' + $portFile + '"'), '--peer'
    ) -WorkingDirectory $root -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $run 'server.out') -RedirectStandardError (Join-Path $run 'server.err')
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $server.Refresh()
        if ($server.HasExited) { throw 'Synthetic server exited before readiness.' }
        if (Test-Path -LiteralPath $portFile) {
            $port = [int]([IO.File]::ReadAllText($portFile).Trim())
            $baseUrl = "http://127.0.0.1:$port"
            try { $health = Invoke-RestMethod "$baseUrl/health" -TimeoutSec 2 } catch { $health = $null }
            if ($health.fixture -eq $true -and $health.peerPort -ge 1) { break }
        }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($health.fixture -ne $true) { throw 'Synthetic server readiness timed out.' }
    $registered = $true
    $launch = WinApp @('run', $layout, '--detach', '--devtools', 'off', '--json') | ConvertFrom-Json
    $previewId = [int]$launch.ProcessId
    $launch | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $run 'process.json')
    WaitPreviewWindow
    [void](WaitUi 'VaultSortChoice')
    Click 'SignInButton'
    Click 'ServerRegion'
    Click '自托管 / 自定义服务器'
    SetValue 'CustomServerUrl' $baseUrl
    SetValue '电子邮箱' 'test@bitwarden.com'
    SetValue '主密码' 'asdfasdfasdf'
    if ($MeasureFolders) { . (Join-Path $PSScriptRoot 'vault-folder-stress-ui-checks.ps1') }
    $initialLoadWatch = [Diagnostics.Stopwatch]::StartNew()
    Click 'PrimaryButton'
    if ($MeasureFolders) {
        WaitFolderStressProperty 'ItemCount' 'Name' '2 个条目'
        $initialLoadWatch.Stop()
        InvokeFolderStressChecks -InitialLoadMs $initialLoadWatch.Elapsed.TotalMilliseconds -Samples $FolderSamples -Label $FolderRunLabel
        Write-Host "PASS: $checks named folder pressure checks. Evidence: $run"
        return
    }
    $tree = WaitUi '保险库已加载'
    if ($CheckLinkedFields) {
        . (Join-Path $PSScriptRoot 'vault-linked-fields-ui-checks.ps1')
        InvokeLinkedFieldUiChecks
        Write-Host "$checks named linked-field UI checks passed. Evidence: $run"
        return
    }
    Check ($tree -match '2 个条目') 'fresh MSIX loads both public synthetic items'
    SetValue 'TextBox' 'login fixture-user'
    $tree = WaitUi '1 个条目'
    Check ($tree -notmatch 'ListItem.*"Synthetic protected item"') 'multiword metadata search ANDs name and subtitle'
    Click 'Synthetic login' 'ListItem'
    [void](WaitUi '通行密钥')
    SetValue 'TextBox' '"synthetic login" fixture-user'
    $tree = WaitUi '通行密钥'
    Check ($tree -match '1 个条目' -and $tree -notmatch '验证主密码') 'equivalent phrase search preserves the open detail'
    SetValue 'TextBox' 'unmatched metadata'
    $tree = WaitUi '没有匹配的条目'
    Check ($tree -notmatch '通行密钥') 'filtering out the selection clears the previous detail'
    SetValue 'TextBox' ''
    ChooseSort '名称（Z → A）'
    $tree = Tree
    Check ($tree.IndexOf('ListItem "Synthetic protected item"') -ge 0 -and $tree.IndexOf('ListItem "Synthetic login"') -ge 0 -and
        $tree.IndexOf('ListItem "Synthetic protected item"') -lt $tree.IndexOf('ListItem "Synthetic login"')) 'descending sorting changes the visible list order'
    ChooseSort '名称（A → Z）'
    $tree = Tree
    Check ($tree.IndexOf('ListItem "Synthetic login"') -ge 0 -and $tree.IndexOf('ListItem "Synthetic protected item"') -ge 0 -and
        $tree.IndexOf('ListItem "Synthetic login"') -lt $tree.IndexOf('ListItem "Synthetic protected item"')) 'ascending sorting restores the visible list order'
    Resize 1000 1500
    if ($IncludeFolders) {
        . (Join-Path $PSScriptRoot 'vault-folder-ui-checks.ps1')
        InvokeFolderUiChecks
    }
    Click 'Synthetic protected item' 'ListItem'
    [void](WaitUi '验证主密码')
    SetValue '主密码' 'asdfasdfasdf'
    Click 'PrimaryButton'
    $tree = WaitUi '修改时间'
    Check ($tree -match 'DetailBackButton' -and $tree -notmatch 'VaultList.*List') 'compact layout shows only the selected detail and its back button'
    ChooseSort '名称（Z → A）'
    $tree = Tree
    Check ($tree -match '修改时间' -and $tree -notmatch '验证主密码') 'sorting an already authorized protected detail does not reprompt'
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'compact-detail.png')))
    Click 'DetailBackButton'
    $tree = WaitUi 'VaultList'
    Check ($tree -notmatch '修改时间') 'back navigation clears sensitive detail and restores the list'
    Keys 'ctrl+f'
    $focus = WinApp @('ui', 'get-focused', '-a', [string]$previewId)
    Check ($focus -match 'Focused: Edit 搜索保险库') 'Ctrl+F focuses search'
    Keys 'ctrl+n'
    [void](WaitUi '新增条目')
    Check $true 'Ctrl+N opens the normal guarded item editor'
    Click 'CloseButton'
    Click 'PART_PaneToggleButton'
    Click 'SettingsItem'
    [void](WaitUi 'ThemeChoice')
    Click 'ThemeChoice'
    Click '深色'
    Check ((WaitValue 'ThemeChoice' '深色') -eq '深色') 'theme preference switches to dark'
    Click 'ClipboardDelayChoice'
    Click '10 秒'
    Check ((WaitValue 'ClipboardDelayChoice' '10 秒') -eq '10 秒') 'clipboard timeout preference switches to ten seconds'
    Click 'KeepInTrayToggle'
    Click 'PART_PaneToggleButton'
    Click 'VaultNavigationItem'
    [void](WaitUi 'VaultList')
    Check ((WaitValue 'VaultSortChoice' '名称（Z → A）') -eq '名称（Z → A）') 'saved sorting survives page recreation'
    Click 'Synthetic login' 'ListItem'
    [void](WaitUi '修改时间')
    Click '复制用户名' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'copy places only the synthetic username on the Windows clipboard'
    $deadline = [DateTime]::UtcNow.AddSeconds(14)
    do {
        if ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    Check ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) 'configured ten-second delay clears owned clipboard data'
    Click '复制用户名' 'Button'
    Start-Sleep -Milliseconds 9300
    # Keep open, sleep and close on the same native thread; PowerShell cmdlets can resume on another thread.
    [SyntheticListWindow]::HoldClipboard(1600)
    $deadline = [DateTime]::UtcNow.AddSeconds(4)
    do {
        if ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    Check ([string]::IsNullOrEmpty((Get-Clipboard -Raw))) 'owned clipboard data is cleared after a native clipboard lock spans its expiry'
    if ($IncludeClipboardStress) {
        . (Join-Path $PSScriptRoot 'vault-clipboard-ui-checks.ps1')
        InvokeClipboardStressChecks
    }
    Click '复制用户名' 'Button'
    Set-Clipboard -Value 'Synthetic other clipboard owner'
    Start-Sleep -Seconds 11
    Check ((Get-Clipboard -Raw) -eq 'Synthetic other clipboard owner') 'clipboard timeout preserves a subsequent writer'
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'compact-dark.png')))
    Keys 'ctrl+e'
    [void](WaitUi '编辑条目')
    Check $true 'Ctrl+E opens the normal guarded edit flow'
    Click 'CloseButton'
    $beforeSync = (Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests
    Keys 'ctrl+r'
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $afterSync = (Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests
        if ($afterSync -gt $beforeSync) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    Check ($afterSync -eq $beforeSync + 1) 'Ctrl+R issues exactly one vault sync request'
    [void](WaitUi '保险库已加载')
    Keys 'ctrl+l'
    $tree = WaitUi '保险库已锁定'
    Check ($tree -notmatch 'ListItem.*"Synthetic') 'Ctrl+L locks and clears the vault list'
    Check ((Get-Clipboard -Raw) -eq 'Synthetic other clipboard owner') 'vault locking also preserves a subsequent clipboard writer'
    $app = Get-Process -Id $previewId
    if (-not $app.CloseMainWindow() -or -not $app.WaitForExit(10000)) { throw 'Synthetic preview did not exit normally.' }
    $launch = WinApp @('run', $layout, '--detach', '--devtools', 'off', '--json') | ConvertFrom-Json
    $previewId = [int]$launch.ProcessId
    WaitPreviewWindow
    [void](WaitUi '保险库已锁定')
    Resize 1000 1500
    Check ((WaitValue 'VaultSortChoice' '名称（Z → A）') -eq '名称（Z → A）') 'sorting preference survives a complete process restart'
    Click 'PART_PaneToggleButton'
    Click 'SettingsItem'
    [void](WaitUi 'ThemeChoice')
    $themeAfterRestart = WaitValue 'ThemeChoice' '深色'
    $clipboardAfterRestart = WaitValue 'ClipboardDelayChoice' '10 秒'
    Tree | Set-Content (Join-Path $run 'settings-after-restart.txt')
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'settings-after-restart.png')))
    Check ($themeAfterRestart -eq '深色') "dark theme preference survives a complete process restart (actual: $themeAfterRestart)"
    Check ($clipboardAfterRestart -eq '10 秒') "clipboard timeout preference survives a complete process restart (actual: $clipboardAfterRestart)"
    "Evidence: $run" | Write-Host
    "$checks UI checks passed." | Write-Host
} finally {
    if ($previewId) {
        $app = Get-Process -Id $previewId -ErrorAction SilentlyContinue
        if ($app) {
            [void]$app.CloseMainWindow()
            if (-not $app.WaitForExit(5000)) { Stop-Process -Id $previewId -Force }
        }
    }
    if ($registered) { Get-AppxPackage -Name $identity | Remove-AppxPackage }
    if ($server) {
        $server.Refresh()
        if (-not $server.HasExited) {
            if ($baseUrl) { try { Invoke-RestMethod -Method Post "$baseUrl/control/stop" -TimeoutSec 2 | Out-Null } catch { } }
            if (-not $server.WaitForExit(5000)) { Stop-Process -Id $server.Id -Force }
        }
        $server.Dispose()
    }
}
