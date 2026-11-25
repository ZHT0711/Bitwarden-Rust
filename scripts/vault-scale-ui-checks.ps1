# Dot-sourced by verify-vault-list-ui.ps1; isolated public SDK account and actual MSIX only.
function WaitVaultScaleCount([string]$Expected) {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $last = ''
    do {
        try {
            $last = WinApp @('ui', 'get-property', 'ItemCount', '-w',
                [string](Get-Process -Id $previewId).MainWindowHandle, '--property', 'Name')
            if ($last -match '(?im)^\s*Name\s*[:=]\s*(.*?)\s*$' -and $Matches[1] -eq $Expected) { return }
        } catch { $last = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    $last | Set-Content (Join-Path $run 'vault-scale-unexpected-count.txt')
    throw "Complete-vault count did not settle: $Expected"
}

function InvokeVaultScaleChecks([double]$InitialLoadMs, [int]$IdleSeconds, [string]$Label) {
    $metrics = [ordered]@{
        completed = $false; label = $Label; items = 10000; initialLoadMs = $InitialLoadMs
        initialLoadScope = 'Sign-in UIA click through 10000-item ready property; includes loopback server, SDK authentication/decryption, CLI and polling. Not cold process startup or frame time.'
        os = [Environment]::OSVersion.VersionString; logicalProcessors = [Environment]::ProcessorCount
        packagedNativeSha256 = (Get-FileHash (Join-Path $layout 'bitwarden_core.dll')).Hash
        probeSourceHashes = [ordered]@{}; probeAssemblySha256 = ''; checks = 0; lockedIdle = @()
    }
    $locked = [Collections.Generic.List[object]]::new()
    function SaveVaultScaleMetrics {
        $metrics.checks = $script:checks
        $metrics.lockedIdle = $locked.ToArray()
        $metrics | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $run 'vault-scale-metrics.json')
    }
    SaveVaultScaleMetrics
    try {
        Check $true 'actual MSIX decrypts and displays 10000 public synthetic items'
        [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'vault-scale-loaded.png')))
        $project = Join-Path $root 'tests/Bitwarden.PerformanceProbe/Bitwarden.PerformanceProbe.csproj'
        & dotnet build $project -c Release *> (Join-Path $run 'probe-build.log')
        if ($LASTEXITCODE) { throw "Performance probe build failed. See $run/probe-build.log" }
        foreach ($source in @('Program.cs', 'NativeInput.cs', 'Bitwarden.PerformanceProbe.csproj')) {
            $metrics.probeSourceHashes[$source] = (Get-FileHash (Join-Path (Split-Path $project) $source)).Hash
        }
        $probe = Join-Path (Split-Path $project) 'bin/Release/net10.0-windows/Bitwarden.PerformanceProbe.dll'
        $metrics.probeAssemblySha256 = (Get-FileHash $probe).Hash
        SaveVaultScaleMetrics
        $hwnd = [string](Get-Process -Id $previewId).MainWindowHandle
        [void](WinApp @('ui', 'focus', '-w', $hwnd))
        & dotnet $probe ([string]$previewId) $hwnd '10000' 'Synthetic entry 09999' (Join-Path $run 'native-probe.json') ([string]$IdleSeconds) *> (Join-Path $run 'probe.log')
        if ($LASTEXITCODE) { throw "Native input/resource probe failed. See $run/probe.log" }
        $result = [IO.File]::ReadAllText((Join-Path $run 'native-probe.json')) | ConvertFrom-Json -Depth 12
        Check ($result.completed -eq $true) 'persistent direct-UIA input/scroll/idle probe completes'
        Check (@($result.samples | Where-Object action -eq 'unicode-input-to-uia-ready').Count -eq 15) '15 burst/20ms/60ms Unicode typing samples preserve all characters and exact AND result'
        Check (@($result.samples | Where-Object action -eq 'scroll-bottom-to-uia-ready').Count -eq 1) 'actual final row of 10000-item virtualized list is fully inside viewport after bottom scrolling'
        [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'vault-scale-after-input.png')))
        SetValue 'TextBox' 'entry 09999'
        WaitVaultScaleCount '1 个条目'
        Click 'Synthetic entry 09999' 'ListItem'
        [void](WaitUi '验证主密码')
        Check $true 'last item in complete vault retains native master-password reprompt'
        Click 'CloseButton'
        SetValue 'TextBox' ''
        WaitVaultScaleCount '10000 个条目'
        Click 'LockButton'
        $lockedTree = WaitUi '保险库已锁定'
        Check ($lockedTree -notmatch 'ListItem.*Synthetic entry') 'locking complete vault removes all visible item metadata'
        $lockedTree | Set-Content (Join-Path $run 'vault-scale-locked-ui.txt')
        [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'vault-scale-locked.png')))
        $cpu = (Get-Process -Id $previewId).TotalProcessorTime.TotalMilliseconds
        $watch = [Diagnostics.Stopwatch]::StartNew()
        # No UIA/provider queries while sampling locked idle; do not force application GC.
        for ($second = 0; $second -lt $IdleSeconds; $second++) {
            Start-Sleep -Seconds 1
            $process = Get-Process -Id $previewId
            $locked.Add([ordered]@{
                stage = "locked-idle-$($second + 1)s"; timestamp = [DateTime]::UtcNow.ToString('o')
                privateBytes = $process.PrivateMemorySize64; workingSetBytes = $process.WorkingSet64
                peakWorkingSetBytes = $process.PeakWorkingSet64; cpuMs = $process.TotalProcessorTime.TotalMilliseconds
            })
        }
        $elapsed = $watch.Elapsed.TotalMilliseconds
        $cpuMs = (Get-Process -Id $previewId).TotalProcessorTime.TotalMilliseconds - $cpu
        $metrics.lockedIdleElapsedMs = $elapsed
        $metrics.lockedIdleCpuMs = $cpuMs
        $metrics.lockedIdleCpuPercentOfOneCore = if ($elapsed -gt 0) { $cpuMs / $elapsed * 100 } else { 0 }
        $metrics.completed = $true
    } finally { SaveVaultScaleMetrics }
}
