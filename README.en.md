<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/bytewarden-banner-dark.png" />
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/bytewarden-banner-light.png" />
  <img alt="Bytewarden logo" src="docs/assets/bytewarden-banner-light.png" width="460" />
</picture>

[![切换到简体中文](https://img.shields.io/badge/README-%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-005AA4?style=flat-square)](./README.md) · **English**

Bytewarden is an unofficial, Bitwarden-compatible Windows desktop client with a Rust core. WinUI 3 powers the current functional preview. A separate [native GPUI desktop preview](apps/Bytewarden.Gpui/README.md) explores a redesigned interface rendered in Rust without a WebView. It now connects to the shared Rust core for sign-in, sync and browsing, but [feature migration and validation](docs/GPUI-MIGRATION.md) are still in progress. It does not replace the WinUI app yet. The project aims for desktop parity before tackling Firefox integration.

[![Windows Validation](https://img.shields.io/github/actions/workflow/status/ZHT0711/Bytewarden/windows.yml?style=for-the-badge&logo=github&label=Windows%20CI)](https://github.com/ZHT0711/Bytewarden/actions/workflows/windows.yml)
[![GPL-3.0](https://img.shields.io/github/license/ZHT0711/Bytewarden.svg?style=for-the-badge)](LICENSE)

> [!IMPORTANT]
> This is an early development preview, not a replacement for your only vault client or backup. Bytewarden is not affiliated with Bitwarden, Inc. Report Bytewarden issues here, not through official Bitwarden support.

## What works in the preview

- **Sign-in and accounts:** Master-password login against the US or EU cloud or a self-hosted server, with authenticator, email or YubiKey two-step login. Switch between up to five isolated accounts. Optional remembered two-step tokens are protected with Windows DPAPI. Plain HTTP is allowed only for loopback servers.
- **Vault browsing and editing:** Browse the eight SDK item types, favorites, archive, trash and nested folders. Search names and displayed subtitles by multiple terms or quoted phrases; no secrets are decrypted for search. Create and edit items, move them in bulk, and manage their lifecycle. Folder rename and deletion do not cascade to unselected descendants.
- **Sync and offline access:** The Rust core decrypts details on demand, enforces master-password reprompts and honors password visibility permissions. Windows DPAPI protects server-encrypted snapshots at rest. The app starts locked; offline snapshots remain read-only until a successful sync. Windows Hello can unlock a local vault but does not replace server authentication or item reprompts.
- **Windows integration:** Adaptive list/detail layouts, light and dark themes, keyboard shortcuts, optional startup and tray behavior, and per-account vault timeout policies. The app attempts to clear only the clipboard content it owns; busy or forcibly terminated processes can prevent clearing.
- **Vault tools:** SDK password and passphrase generation, offline TOTP, encrypted attachments, text and file Send, and personal vault import/export in Bitwarden JSON and CSV. CSV cannot preserve all item data; prefer password-protected JSON for backups. Backups produced by this client do not include attachment files, trash or organization vaults.
- **Passkeys:** View metadata for saved passkeys. The experimental Windows provider still needs end-to-end validation on real websites. Passkey private material is not returned to the WinUI detail view.

## Project layout and scope

| Path | Role |
| --- | --- |
| [`crates/bytewarden-core`](crates/bytewarden-core) | Rust core and C ABI for the desktop app |
| [`apps/Bytewarden.WinUI`](apps/Bytewarden.WinUI) | Windows desktop client (.NET 10 / Windows App SDK) |
| [`tests`](tests) | Managed checks, native interop and the local synthetic fixture server |
| [`scripts`](scripts) | Build, UI acceptance and performance tooling |

The current comparison baseline is Bitwarden Desktop 2026.11.0. This does not imply full parity: SSO, Duo/WebAuthn login, V2 login and reprompts, live cloud and organization testing, and Firefox integration remain open. See the [desktop parity matrix](docs/PARITY.md) and [performance notes](docs/PERFORMANCE.md) (Chinese) for details and test limits. The upstream Rust SDK is pinned to `56c7617b23501af70221094cdee59e10b2a9d8d2`; see [third-party notices](THIRD_PARTY_NOTICES.md).

## Build and run on Windows x64

Install stable Rust for `x86_64-pc-windows-msvc`, the .NET 10 SDK, MSVC x64 build tools and a Windows SDK. The `winapp` CLI is needed to run the preview as shown below; synthetic-server checks also require Python 3.10+.

Run from the repository root in PowerShell. Dot-source the Rust build script so that subsequent Cargo commands use the same MSVC and SDK environment:

```powershell
. ./scripts/build-rust-core.ps1
cargo test --locked -p bytewarden-core
cargo clippy --locked -p bytewarden-core --all-targets -- -D warnings
dotnet run --project tests/Bytewarden.Interop.Tests -- target/debug/bytewarden_core.dll
dotnet build apps/Bytewarden.WinUI/Bytewarden.WinUI.csproj -p:Platform=x64 -p:GenerateAppxPackageOnBuild=true -p:AppxPackageSigningEnabled=false
winapp run apps/Bytewarden.WinUI/Bytewarden.WinUI.csproj --no-build -p Platform=x64
```

The default `ZHT0711.BytewardenLocalPreview` package does not replace the official app. Use the same `Platform=x64` when building and running. Leave `--unique-identity` off for regular previews to avoid duplicate Start menu entries.

## CI and preview packages

The [Windows validation workflow](https://github.com/ZHT0711/Bytewarden/actions/workflows/windows.yml) runs Rust formatting, tests and Clippy, fixture server self-tests, and managed/native interop against a Release DLL. It builds an x64 Release MSIX with the .NET runtime included and verifies that the packaged native DLL matches the DLL tested in CI.

Download `Bytewarden-unsigned-msix-x64` from the **Artifacts** section of a successful run. The run summary lists SHA-256 hashes. These are unsigned development artifacts, not signed installers. Interactive UI acceptance runs separately on a Windows desktop with synthetic data; see the [Chinese test instructions](README.md#自动化测试与压力验收).

## Feedback and license

Search [GitHub Issues](https://github.com/ZHT0711/Bytewarden/issues) before filing a report. Include the steps to reproduce and the version information from **About → Copy version information**. Never include credentials, tokens or vault content.

The project, formerly called **Bitwarden-Rust**, is licensed under [GPL-3.0](LICENSE). Keep independent backups and do not rely on this preview as the sole copy of important data.
