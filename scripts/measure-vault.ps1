param(
    [ValidateSet("Debug", "Release")]
    [string] $Profile = "Release"
)

$ErrorActionPreference = "Stop"

# Use the same native toolchain environment as the app, including C dependency headers.
. (Join-Path $PSScriptRoot "build-rust-core.ps1") -Profile $Profile
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$manifest = Join-Path $PSScriptRoot "../Cargo.toml"
$cargoArgs = @("test", "--locked", "--manifest-path", $manifest, "-p", "bitwarden-core@0.2.0")
if ($Profile -eq "Release") { $cargoArgs += "--release" }
$cargoArgs += @("measure_confirmed_write_projection", "--", "--ignored", "--nocapture", "--test-threads=1")
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
