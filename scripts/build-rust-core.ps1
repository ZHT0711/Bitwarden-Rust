param(
    [ValidateSet("Debug", "Release")]
    [string] $Profile = "Debug"
)

$ErrorActionPreference = "Stop"
$vswherePath = "C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
if (-not (Test-Path $vswherePath)) {
    throw "Visual Studio Installer's vswhere.exe was not found. Install the Visual C++ x64/x86 build tools."
}

$vsPath = & $vswherePath -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsPath) {
    throw "The Visual C++ x64/x86 build tools are missing."
}

$toolset = Get-ChildItem (Join-Path $vsPath "VC/Tools/MSVC") -Directory | Sort-Object Name -Descending | Select-Object -First 1
if (-not $toolset) {
    throw "No MSVC toolset was found in the Visual Studio installation."
}

$roots = @("D:/Windows Kits/10", "C:/Program Files (x86)/Windows Kits/10", "C:/Program Files/Windows Kits/10")
$sdkRoot = $roots | Where-Object { Test-Path (Join-Path $_ "Lib") } | Select-Object -First 1
if (-not $sdkRoot) {
    throw "A Windows 10/11 SDK was not found."
}

$sdkVersion = Get-ChildItem (Join-Path $sdkRoot "Lib") -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName "um/x64/kernel32.lib") } |
    Sort-Object Name -Descending |
    Select-Object -First 1
if (-not $sdkVersion) {
    throw "The Windows SDK libraries for x64 were not found."
}

$msvcRoot = $toolset.FullName
$msvcBin = Join-Path $msvcRoot "bin/Hostx64/x64"
$sdkVersionRoot = $sdkVersion.FullName
$cargoBin = Join-Path $env:USERPROFILE ".cargo/bin"
$pathEntries = @($msvcBin, (Join-Path $sdkRoot "bin/$($sdkVersion.Name)/x64"), $cargoBin) +
    ($env:Path -split ";" | Where-Object { $_ -and $_ -notlike "C:/Program Files/coreutils/bin*" })
$env:Path = $pathEntries -join ";"
$env:LIB = @(
    (Join-Path $msvcRoot "lib/x64"),
    (Join-Path $sdkVersionRoot "um/x64"),
    (Join-Path $sdkVersionRoot "ucrt/x64")
) -join ";"
# Native dependencies such as ring and SQLite compile C sources on a clean build.
# Configure their headers as well as the Rust linker's libraries, even outside a VS shell.
$sdkIncludeRoot = Join-Path $sdkRoot "Include/$($sdkVersion.Name)"
$env:INCLUDE = @(
    (Join-Path $msvcRoot "include"),
    (Join-Path $sdkIncludeRoot "ucrt"),
    (Join-Path $sdkIncludeRoot "shared"),
    (Join-Path $sdkIncludeRoot "um"),
    (Join-Path $sdkIncludeRoot "winrt"),
    (Join-Path $sdkIncludeRoot "cppwinrt")
) -join ";"

$workspaceManifest = Join-Path $PSScriptRoot "../Cargo.toml"
$cargoArgs = @("build", "--locked", "-p", "bytewarden-core", "--manifest-path", $workspaceManifest)
if ($Profile -eq "Release") {
    $cargoArgs += "--release"
}

& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
