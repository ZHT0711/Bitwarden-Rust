param(
    [string]$Library = 'target/release/bytewarden_core.dll',
    [string]$Fixture = 'work/linked-fields-fixture.json',
    [string]$PythonExe = 'python'
)

# Public SDK fixture only. The host removes the isolated cache after the native process exits,
# because the Rust cache's exclusive writer lock deliberately lasts for the process lifetime.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'fixture-python.ps1')
$pythonPath = Resolve-FixturePython $PythonExe
$root = Split-Path $PSScriptRoot -Parent
$libraryPath = (Resolve-Path (Join-Path $root $Library)).Path
$fixturePath = (Resolve-Path (Join-Path $root $Fixture)).Path
$fixtureData = [IO.File]::ReadAllText($fixturePath) | ConvertFrom-Json
if ($fixtureData.linkedFieldFixture.synthetic -ne $true -or $fixtureData.ciphers.Count -ne 6) {
    throw 'Expected the dedicated six-cipher public linked-field fixture.'
}
$run = Join-Path $root ('work/linked-native-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $run | Out-Null
$cache = [IO.Directory]::CreateTempSubdirectory('bytewarden-linked-check-')
$server = $null
$baseUrl = $null
$log = Join-Path $run 'checks.log'
try {
    $portFile = Join-Path $run 'port'
    $server = Start-Process -FilePath $pythonPath -ArgumentList @(
        '-u', 'tests/fixture_server.py', '--fixture', ('"' + $fixturePath + '"'),
        '--port-file', ('"' + $portFile + '"'), '--peer'
    ) -WorkingDirectory $root -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $run 'server.out') -RedirectStandardError (Join-Path $run 'server.err')
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $server.Refresh()
        if ($server.HasExited) { throw 'Linked fixture exited before readiness.' }
        if (Test-Path -LiteralPath $portFile) {
            $baseUrl = 'http://127.0.0.1:' + [IO.File]::ReadAllText($portFile).Trim()
            try { $health = Invoke-RestMethod "$baseUrl/health" -TimeoutSec 2 } catch { $health = $null }
            if ($health.fixture -eq $true) { break }
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($health.fixture -ne $true) { throw 'Linked fixture never became ready.' }
    [ordered]@{
        pythonExecutable = $pythonPath
        dllSha256 = (Get-FileHash -LiteralPath $libraryPath).Hash
        fixtureSha256 = (Get-FileHash -LiteralPath $fixturePath).Hash
        testSourceSha256 = (Get-FileHash (Join-Path $root 'tests/Bytewarden.Interop.Tests/NativeLinkedFieldChecks.cs')).Hash
        hostSourceSha256 = (Get-FileHash $PSCommandPath).Hash
        server = $baseUrl
        cacheDirectory = $cache.FullName
    } | ConvertTo-Json | Set-Content (Join-Path $run 'inputs.json')
    & dotnet run --project (Join-Path $root 'tests/Bytewarden.Interop.Tests') -c Release -- --linked-fields $libraryPath $baseUrl $cache.FullName 2>&1 |
        Tee-Object -FilePath $log | Select-Object -Last 16 | Write-Host
    if ($LASTEXITCODE) { throw "Native linked checks failed. Evidence: $run" }
    Write-Host "Evidence: $run"
} finally {
    if ($server) {
        $server.Refresh()
        if (-not $server.HasExited) {
            if ($baseUrl) { try { Invoke-RestMethod -Method Post "$baseUrl/control/stop" -TimeoutSec 2 | Out-Null } catch {} }
            if (-not $server.WaitForExit(5000)) { Stop-Process -Id $server.Id -Force }
        }
        $server.Dispose()
    }
    $cache.Delete($true)
}
