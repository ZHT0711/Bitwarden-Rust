param(
    [Parameter(Mandatory)][string]$Package,
    [string]$Library = 'target/release/bytewarden_core.dll'
)

$ErrorActionPreference = 'Stop'
$packagePath = (Resolve-Path -LiteralPath $Package).ProviderPath
$libraryHash = (Get-FileHash -LiteralPath $Library -Algorithm SHA256).Hash
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($packagePath)
try {
    foreach ($name in @('AppxManifest.xml', 'Bytewarden.WinUI.exe', 'bytewarden_core.dll', 'coreclr.dll', 'hostfxr.dll')) {
        if (-not $archive.GetEntry($name)) { throw "Release MSIX is missing $name." }
    }
    if ($archive.GetEntry('AppxSignature.p7x')) { throw 'Expected an unsigned development preview.' }
    $reader = [IO.StreamReader]::new($archive.GetEntry('AppxManifest.xml').Open())
    try { $manifest = [xml]$reader.ReadToEnd() } finally { $reader.Dispose() }
    $identity = $manifest.Package.Identity
    if ($identity.Name -cne 'ZHT0711.BytewardenLocalPreview' -or $identity.ProcessorArchitecture -cne 'x64') {
        throw 'Expected the Bytewarden x64 local preview identity.'
    }
    $core = $archive.GetEntry('bytewarden_core.dll').Open()
    try { $packagedHash = (Get-FileHash -InputStream $core -Algorithm SHA256).Hash } finally { $core.Dispose() }
    if ($packagedHash -cne $libraryHash) { throw 'Packaged Rust DLL does not match the Release DLL used for interop validation.' }
} finally { $archive.Dispose() }

[pscustomobject]@{
    FileName = [IO.Path]::GetFileName($packagePath)
    PackageVersion = $identity.Version
    PackageSha256 = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash
    LibrarySha256 = $packagedHash
}
