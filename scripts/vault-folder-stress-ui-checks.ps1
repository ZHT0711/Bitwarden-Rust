# Dot-sourced by verify-vault-list-ui.ps1. Public SDK fixture and isolated test identity only.
# Timings include WinApp/UIA command latency, polling and application work, NOT frame times.
function WaitFolderStressProperty([string]$Selector, [string]$Property, [string]$Pattern, [switch]$Popup) {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $last = ''
    do {
        try {
            $target = if ($Popup) { @('-a', [string]$previewId) } else { @('-w', [string](Get-Process -Id $previewId).MainWindowHandle) }
            $last = WinApp (@('ui', 'get-property', $Selector) + $target + @('--property', $Property))
            if ($Property -eq 'Name') {
                if ($last -match '(?im)^\s*Name\s*[:=]\s*(.*?)\s*$' -and $Matches[1] -match ('\A(?:' + $Pattern + ')\z')) { return }
            } elseif ($last -match $Pattern) { return }
        } catch { $last = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    $last | Set-Content (Join-Path $run 'folder-stress-unexpected-property.txt')
    throw "Folder stress property did not settle: $Selector / $Property / $Pattern"
}

# JSON avoids the CLI's human-readable line wrapping; never normalize spaces in real folder names.
function WaitFolderStressValue([string]$Selector, [string]$Expected) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $raw = WinApp @('ui', 'get-value', $Selector, '-w',
            [string](Get-Process -Id $previewId).MainWindowHandle, '--json')
        $result = $raw | ConvertFrom-Json
        $value = if ($result -is [string]) { $result } else { $result.text }
        if ($value -isnot [string]) {
            $raw | Set-Content (Join-Path $run 'folder-stress-value-json.txt')
            throw 'Unexpected JSON value response; inspect the saved raw response.'
        }
        if ([string]::Equals($value, $Expected, [StringComparison]::Ordinal)) { return $value }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    $raw | Set-Content (Join-Path $run 'folder-stress-value-json.txt')
    throw "Folder value differs: $Selector; expected $($Expected.Length) characters, actual $($value.Length)."
}

function InvokeFolderStressChecks([double]$InitialLoadMs, [int]$Samples, [string]$Label) {
    $inputFixture = [IO.File]::ReadAllText($fixturePath) | ConvertFrom-Json -Depth 12
    $meta = $inputFixture.folderStress
    if (-not $meta -or $meta.scenario -notin @('flat','branching','deep','long-orphan')) {
        throw 'Pressure mode requires the explicit public SDK folderStress fixture.'
    }
    $measurements = [Collections.Generic.List[object]]::new()
    $memory = [Collections.Generic.List[object]]::new()
    $metrics = [ordered]@{
        label = $Label; completed = $false; scenario = $meta; samples = $Samples
        initialLoadMs = $InitialLoadMs
        timingScope = 'Wall clock from UIA action through visible ready property; includes CLI/polling overhead. Not frame time.'
        memoryScope = 'Actual MSIX process snapshots and OS peak working set; sampled private bytes are not an exact peak.'
        os = [Environment]::OSVersion.VersionString; logicalProcessors = [Environment]::ProcessorCount
        checks = 0; measurements = @(); memory = @()
    }
    function SaveFolderStressMetrics {
        $metrics.measurements = $measurements.ToArray()
        $metrics.memory = $memory.ToArray()
        $metrics.checks = $script:checks
        $metrics | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $run 'folder-stress-metrics.json')
    }
    function CaptureFolderStressMemory([string]$Stage) {
        $process = Get-Process -Id $previewId
        $process.Refresh()
        $memory.Add([ordered]@{
            stage = $Stage; timestamp = [DateTime]::UtcNow.ToString('o')
            privateBytes = $process.PrivateMemorySize64; workingSetBytes = $process.WorkingSet64
            peakWorkingSetBytes = $process.PeakWorkingSet64; peakPagedMemoryBytes = $process.PeakPagedMemorySize64
            cpuMs = $process.TotalProcessorTime.TotalMilliseconds
        })
        SaveFolderStressMetrics
    }
    function MeasureFolderStress([string]$Name, [int]$Iteration, [scriptblock]$Action, [scriptblock]$Ready) {
        $cpu = (Get-Process -Id $previewId).TotalProcessorTime.TotalMilliseconds
        $watch = [Diagnostics.Stopwatch]::StartNew()
        & $Action
        & $Ready
        $watch.Stop()
        $measurements.Add([ordered]@{
            action = $Name; iteration = $Iteration; elapsedMs = $watch.Elapsed.TotalMilliseconds
            applicationCpuMs = (Get-Process -Id $previewId).TotalProcessorTime.TotalMilliseconds - $cpu
        })
        CaptureFolderStressMemory "$Name-$Iteration"
    }
    function NavigateFolderStress([string]$Selector) {
        $visible = $false
        try {
            $value = WinApp @('ui', 'get-property', $Selector, '-w',
                [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'IsOffscreen')
            $visible = $value -match '(?im)^\s*IsOffscreen\s*[:=]\s*False\s*$'
        } catch { }
        if (-not $visible) { Click 'PART_PaneToggleButton' }
        Click $Selector
    }
    function FolderStressInspect([string]$Selector, [string]$File) {
        $tree = WinApp @('ui', 'inspect', $Selector, '-w',
            [string](Get-Process -Id $previewId).MainWindowHandle, '--depth', '5')
        $tree | Set-Content (Join-Path $run $File)
        return $tree
    }
    function CheckFolderStressItems([string]$Expected, [string]$Excluded, [string]$Name) {
        $tree = FolderStressInspect 'VaultList' ($Name + '.txt')
        Check ($tree.Contains('ListItem "' + $Expected + '"') -and -not $tree.Contains('ListItem "' + $Excluded + '"')) $Name
    }
    function FolderStressFilterAction([string]$Action) {
        [void](WinApp @('ui', 'invoke', 'FolderFilter', '-w',
            [string](Get-Process -Id $previewId).MainWindowHandle, '--type', 'ComboBox', '--action', $Action))
    }
    try {
        CaptureFolderStressMemory 'loaded'
        Check $true 'folder stress MSIX loads both SDK encrypted synthetic items'
        $old = [SyntheticListWindow]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
        try {
            $wide = [Math]::Min(2600, [SyntheticListWindow]::GetSystemMetrics(0) - 200)
            $height = [Math]::Min(1500, [SyntheticListWindow]::GetSystemMetrics(1) - 200)
        } finally { [void][SyntheticListWindow]::SetThreadDpiAwarenessContext($old) }
        $metrics.windowPixels = [ordered]@{ wide = $wide; compact = [Math]::Min(1000, $wide); height = $height }
        Resize $wide $height
        $rootCount = [regex]::Escape("$($meta.roots) 个顶层文件夹，共 $($meta.folders) 个文件夹")
        for ($iteration = 0; $iteration -lt $Samples; $iteration++) {
            MeasureFolderStress 'manager' $iteration {
                NavigateFolderStress 'FolderManagementNavigationItem'
            } { WaitFolderStressProperty 'FolderCount' 'Name' $rootCount }
            Check $true "folder stress $($meta.scenario) root and total counts $iteration"
            if ($iteration -eq 0) {
                [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                    '-o', (Join-Path $run 'folder-stress-roots-wide.png')))
                if ($meta.roots -gt 1) {
                    MeasureFolderStress 'roots-scroll-bottom' $iteration {
                        [void](WinApp @('ui', 'scroll', 'FolderList', '-w',
                            [string](Get-Process -Id $previewId).MainWindowHandle, '--to', 'bottom'))
                    } {}
                    [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                        '-o', (Join-Path $run 'folder-stress-roots-bottom.png')))
                    if ($meta.scenario -eq 'flat') {
                        WaitFolderStressProperty ("FolderOpen_" + $meta.targetId) 'IsOffscreen' '(?im)^\s*IsOffscreen\s*[:=]\s*False\s*$'
                        Check $true 'flat 10000-folder list realizes its final real row after scrolling'
                    }
                    [void](WinApp @('ui', 'scroll', 'FolderList', '-w',
                        [string](Get-Process -Id $previewId).MainWindowHandle, '--to', 'top'))
                }
            }
            MeasureFolderStress 'root-open' $iteration {
                Click ("FolderOpen_" + $meta.rootId) 'Hyperlink'
            } { WaitFolderStressProperty 'ItemCount' 'Name' '1 个条目' }
            CheckFolderStressItems 'Synthetic protected item' 'Synthetic login' "root-exact-membership-$iteration"
            if ($meta.scenario -eq 'long-orphan' -and $iteration -eq 0) {
                $children = FolderStressInspect 'ChildFolderList' 'folder-stress-joined-child.txt'
                Check ([regex]::Matches($children, '(?m)^\s*.*\bListItem\b').Count -eq 1) 'long missing path remains one real child with no invented parents'
            }
            NavigateFolderStress 'FolderManagementNavigationItem'
            WaitFolderStressProperty 'FolderCount' 'Name' $rootCount
            MeasureFolderStress 'search-leaf' $iteration { SetValue 'TextBox' ([string]$meta.search) } {
                WaitFolderStressProperty 'FolderCount' 'Name' '1 个匹配文件夹'
            }
            MeasureFolderStress 'leaf-open' $iteration {
                Click ("FolderOpen_" + $meta.targetId) 'Hyperlink'
            } { WaitFolderStressProperty 'ItemCount' 'Name' '1 个条目' }
            Check ((WaitFolderStressValue 'FolderFilter' ([string]$meta.targetName)) -eq $meta.targetName) "leaf keeps exact full folder name $iteration"
            CheckFolderStressItems 'Synthetic login' 'Synthetic protected item' "leaf-exact-membership-$iteration"
            if ($iteration -eq 0) {
                [void](FolderStressInspect 'FolderCrumbs' 'folder-stress-leaf-breadcrumbs.txt')
                [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                    '-o', (Join-Path $run 'folder-stress-leaf-wide.png')))
            }
            MeasureFolderStress 'filter-open' $iteration { FolderStressFilterAction 'expand' } {
                WaitFolderStressProperty 'FolderFilter' 'ExpandCollapseState' 'Expanded'
            }
            if ($iteration -eq 0) {
                [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId,
                    '-o', (Join-Path $run 'folder-stress-filter-popup.png')))
            }
            FolderStressFilterAction 'collapse'
            WaitFolderStressProperty 'FolderFilter' 'ExpandCollapseState' 'Collapsed'
            Check $true "full folder filter opens and closes at actual scale $iteration"
        }
        # Extra correctness coverage, not part of the paired home/navigation memory peak.
        CaptureFolderStressMemory 'paired-navigation-end'
        MeasureFolderStress 'editor-open' 0 { Click 'NewItemButton' } {
            [void](WaitFolderStressValue 'VaultEditorFolder' ([string]$meta.targetName))
        }
        Check ([string]::Equals((WaitFolderStressValue 'VaultEditorFolder' ([string]$meta.targetName)), $meta.targetName, [StringComparison]::Ordinal)) `
            'new item draft inherits the selected exact leaf folder ID rather than an unfiled default'
        CaptureFolderStressMemory 'extra-editor-open'
        MeasureFolderStress 'editor-folder-open' 0 {
            [void](WinApp @('ui', 'invoke', 'VaultEditorFolder', '-w',
                [string](Get-Process -Id $previewId).MainWindowHandle, '--type', 'ComboBox', '--action', 'expand'))
        } { WaitFolderStressProperty 'VaultEditorFolder' 'ExpandCollapseState' 'Expanded' }
        [void](WinApp @('ui', 'invoke', 'VaultEditorFolder', '-w',
            [string](Get-Process -Id $previewId).MainWindowHandle, '--type', 'ComboBox', '--action', 'collapse'))
        Check $true 'editor folder picker opens and closes with the full folder fixture'
        Click 'CloseButton'
        WaitFolderStressProperty 'FolderFilter' 'IsEnabled' '(?im)^\s*IsEnabled\s*[:=]\s*True\s*$'
        Click 'Synthetic login' 'ListItem'
        WaitFolderStressProperty 'EditItemButton' 'IsEnabled' '(?im)^\s*IsEnabled\s*[:=]\s*True\s*$'
        MeasureFolderStress 'move-dialog-open' 0 {
            Click 'MoreButton'
            [void](WinApp @('ui', 'invoke', 'MoveItemsButton', '-a', [string]$previewId, '--type', 'Button'))
        } { [void](WaitFolderStressValue '目标文件夹' '无文件夹') }
        MeasureFolderStress 'move-folder-open' 0 {
            [void](WinApp @('ui', 'invoke', '目标文件夹', '-w',
                [string](Get-Process -Id $previewId).MainWindowHandle, '--type', 'ComboBox', '--action', 'expand'))
        } { WaitFolderStressProperty '目标文件夹' 'ExpandCollapseState' 'Expanded' }
        $popupTree = WinApp @('ui', 'inspect', '-a', [string]$previewId, '--depth', '5')
        $popupTree | Set-Content (Join-Path $run 'move-picker-popup.txt')
        $popupMatch = [regex]::Match($popupTree, '(?m)^--- HWND (\d+): "PopupHost"')
        if (-not $popupMatch.Success) { throw 'The scaled move picker did not expose its own PopupHost window.' }
        $popupHwnd = $popupMatch.Groups[1].Value
        # Tiny orphan popups have no scrollbar. Target a stable real ID, not a CLI-truncated
        # name or ephemeral UIA slug: the orphan's full name exceeds Windows' argv limit.
        if ($meta.scenario -ne 'long-orphan') {
            $pickerEnd = if ($meta.scenario -eq 'branching') { 'top' } else { 'bottom' }
            [void](WinApp @('ui', 'scroll', '目标文件夹', '-w', $popupHwnd, '--to', $pickerEnd))
        }
        [void](WinApp @('ui', 'invoke', ("FolderChoice_" + $meta.targetId), '-w', $popupHwnd))
        Check ([string]::Equals((WaitFolderStressValue '目标文件夹' ([string]$meta.targetName)), $meta.targetName, [StringComparison]::Ordinal)) `
            'move picker preserves exact full-name leaf identity at actual folder scale'
        Click 'CloseButton'
        WaitFolderStressProperty 'FolderFilter' 'IsEnabled' '(?im)^\s*IsEnabled\s*[:=]\s*True\s*$'
        SetValue 'TextBox' 'unmatched synthetic metadata'
        WaitFolderStressProperty 'ItemCount' 'Name' '0 个条目'
        SetValue 'TextBox' ''
        WaitFolderStressProperty 'ItemCount' 'Name' '1 个条目'
        CheckFolderStressItems 'Synthetic login' 'Synthetic protected item' 'canceling scaled move preserves the original exact leaf membership'
        Resize ([Math]::Min(1000, $wide)) $height
        WaitFolderStressProperty 'ItemCount' 'Name' '1 个条目'
        [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
            '-o', (Join-Path $run 'folder-stress-leaf-compact.png')))
        Check ((WaitFolderStressValue 'FolderFilter' ([string]$meta.targetName)) -eq $meta.targetName) 'compact layout preserves full-name exact folder selection'
        if ($meta.scenario -eq 'deep') {
            MeasureFolderStress 'deep-parent-breadcrumb' 0 { Click 'Depth0998' 'Button' } {
                WaitFolderStressProperty 'ItemCount' 'Name' '0 个条目'
            }
            Check ((WaitFolderStressValue 'FolderFilter' (($meta.targetName -split '/')[0..998] -join '/')) -ne $meta.targetName) '1000-level trail navigates to its real parent without leaking leaf membership'
            $rects = @{}
            foreach ($selector in 'EmptyMessage', 'VaultList') {
                $raw = WinApp @('ui', 'get-property', $selector, '-w',
                    [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'BoundingRectangle', '--json')
                $raw | Set-Content (Join-Path $run ("short-window-$selector-rect.json"))
                $rects[$selector] = ($raw | ConvertFrom-Json).element
            }
            [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                '-o', (Join-Path $run 'short-window-empty-parent.png')))
            $text = $rects.EmptyMessage
            $pane = $rects.VaultList
            $scale = [SyntheticListWindow]::GetDpiForWindow((Get-Process -Id $previewId).MainWindowHandle) / 96.0
            Check (-not $text.isOffscreen -and $text.height -ge 18 * $scale -and
                $text.x -ge $pane.x -and $text.y -ge $pane.y -and
                $text.x + $text.width -le $pane.x + $pane.width + 1 -and
                $text.y + $text.height -le $pane.y + $pane.height + 1) `
                'short empty parent shows an entire text line inside the list viewport, not merely a clipped UIA element'
            foreach ($size in @(@(1000, 1100), @(1600, 1000), @(1000, 1380))) {
                Resize $size[0] $size[1]
                Start-Sleep -Milliseconds 250
                $bounds = @{}
                foreach ($selector in 'EmptyMessage', 'VaultList') {
                    $raw = WinApp @('ui', 'get-property', $selector, '-w',
                        [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'BoundingRectangle', '--json')
                    $raw | Set-Content (Join-Path $run ("short-$($size[0])x$($size[1])-$selector.json"))
                    $bounds[$selector] = ($raw | ConvertFrom-Json).element
                }
                [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                    '-o', (Join-Path $run "short-$($size[0])x$($size[1])-empty.png")))
                $text = $bounds.EmptyMessage
                $pane = $bounds.VaultList
                $oldDpi = [SyntheticListWindow]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
                try {
                    $hwnd = (Get-Process -Id $previewId).MainWindowHandle
                    $client = [SyntheticListWindow+Rect]::new()
                    $origin = [SyntheticListWindow+Point]::new()
                    if (-not [SyntheticListWindow]::GetClientRect($hwnd, [ref]$client) -or
                        -not [SyntheticListWindow]::ClientToScreen($hwnd, [ref]$origin)) { throw 'Cannot get physical client bounds.' }
                    $window = @{ x = $origin.x; y = $origin.y; width = $client.right; height = $client.bottom }
                } finally { [void][SyntheticListWindow]::SetThreadDpiAwarenessContext($oldDpi) }
                $window | ConvertTo-Json | Set-Content (Join-Path $run "short-$($size[0])x$($size[1])-client.json")
                Check (-not $text.isOffscreen -and $text.height -ge 18 * $scale -and $pane.height -ge 48 * $scale -and
                    $text.x -ge $pane.x -and $text.y -ge $pane.y -and
                    $text.x + $text.width -le $pane.x + $pane.width + 1 -and
                    $text.y + $text.height -le $pane.y + $pane.height + 1 -and
                    $pane.y + $pane.height -le $window.y + $window.height + 1) `
                    "short $($size[0])x$($size[1]) empty text and usable list viewport are fully inside the actual client window"
                [void](WinApp @('ui', 'scroll', 'VaultHeaderScroll', '-w', [string]$hwnd, '--to', 'bottom'))
                $header = (WinApp @('ui', 'get-property', 'VaultHeaderScroll', '-w', [string]$hwnd,
                    '--property', 'BoundingRectangle', '--json') | ConvertFrom-Json).element
                foreach ($selector in 'FolderAncestorsButton', 'ChildFolderList') {
                    $raw = WinApp @('ui', 'get-property', $selector, '-w', [string]$hwnd,
                        '--property', 'BoundingRectangle', '--json')
                    $raw | Set-Content (Join-Path $run "short-$($size[0])x$($size[1])-$selector.json")
                    $control = ($raw | ConvertFrom-Json).element
                    Check (-not $control.isOffscreen -and $control.height -ge 30 * $scale -and
                        $control.x -ge $header.x -and $control.x + $control.width -le $header.x + $header.width + 1 -and
                        $control.y -ge $header.y -and $control.y + $control.height -le $header.y + $header.height + 1 -and
                        $control.y + $control.height -le $window.y + $window.height + 1) `
                        "short $($size[0])x$($size[1]) $selector remains fully reachable by header scrolling"
                }
                [void](WinApp @('ui', 'screenshot', '-w', [string]$hwnd,
                    '-o', (Join-Path $run "short-$($size[0])x$($size[1])-navigation.png")))
            }
            Resize ([Math]::Min(1000, $wide)) $height
            Click 'FolderAncestorsButton'
            [void](WinApp @('ui', 'scroll', 'FolderAncestorList', '-a', [string]$previewId, '--to', 'bottom'))
            # Parent 998 has early real ancestors 0..990; the last eight stay in the breadcrumb.
            WaitFolderStressProperty ("FolderOpen_" + $inputFixture.folders[990].id) 'IsOffscreen' '(?im)^\s*IsOffscreen\s*[:=]\s*False\s*$' -Popup
            Check $true 'early ancestor menu realizes its last omitted real ancestor after scrolling'
            [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId,
                '-o', (Join-Path $run 'folder-stress-earlier-ancestors.png')))
            [void](WinApp @('ui', 'scroll', 'FolderAncestorList', '-a', [string]$previewId, '--to', 'top'))
            [void](WinApp @('ui', 'invoke', ("FolderOpen_" + $meta.rootId), '-a', [string]$previewId, '--type', 'Hyperlink'))
            Check ([string]::Equals((WaitFolderStressValue 'FolderFilter' ([string]$meta.rootName)), $meta.rootName, [StringComparison]::Ordinal)) `
                'omitted real root remains navigable through the ancestor menu without a synthesized folder'
            CheckFolderStressItems 'Synthetic protected item' 'Synthetic login' 'early-ancestor-exact-membership'
        }
        CaptureFolderStressMemory 'before-lock'
        # Invoke this exact isolated window's control; never send input into another foreground app.
        MeasureFolderStress 'lock' 0 { Click 'LockButton' } {
            WaitFolderStressProperty 'ItemCount' 'Name' '0 个条目'
        }
        Check ((WaitFolderStressValue 'FolderFilter' '所有文件夹') -eq '所有文件夹') 'locking clears selected long-path metadata from the filter'
        if ($meta.scenario -eq 'deep') {
            foreach ($size in @(@(1000, 1100), @(1600, 1000), @(1000, 1380))) {
                Resize $size[0] $size[1]
                Start-Sleep -Milliseconds 250
                $pane = (WinApp @('ui', 'get-property', 'VaultList', '-w',
                    [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'BoundingRectangle', '--json') | ConvertFrom-Json).element
                foreach ($selector in 'EmptyMessage', 'ConnectButton') {
                    $raw = WinApp @('ui', 'get-property', $selector, '-w',
                        [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'BoundingRectangle', '--json')
                    $raw | Set-Content (Join-Path $run "locked-short-$($size[0])x$($size[1])-$selector.json")
                    $control = ($raw | ConvertFrom-Json).element
                    Check (-not $control.isOffscreen -and $control.height -ge 18 * $scale -and
                        $control.x -ge $pane.x -and $control.x + $control.width -le $pane.x + $pane.width + 1 -and
                        $control.y -ge $pane.y -and $control.y + $control.height -le $pane.y + $pane.height + 1) `
                        "locked short $($size[0])x$($size[1]) $selector is fully visible inside the list viewport"
                }
                [void](WinApp @('ui', 'screenshot', '-w', [string](Get-Process -Id $previewId).MainWindowHandle,
                    '-o', (Join-Path $run "locked-short-$($size[0])x$($size[1]).png")))
            }
        }
        Start-Sleep -Seconds 1
        CaptureFolderStressMemory 'after-lock-idle'
        $metrics.completed = $true
    } finally { SaveFolderStressMetrics }
    $measurements | ForEach-Object { [pscustomobject]$_ } | Group-Object action | ForEach-Object {
        $ordered = @($_.Group.elapsedMs | Sort-Object)
        [pscustomobject]@{ action = $_.Name; samples = $ordered.Count; medianMs = [Math]::Round($ordered[[int][Math]::Floor($ordered.Count / 2)], 2); maxMs = [Math]::Round($ordered[-1], 2) }
    } | Format-Table -AutoSize
}
