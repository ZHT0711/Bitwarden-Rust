# Third-party notices

## Bitwarden Rust SDK

This application links the official `bitwarden-core`, `bitwarden-auth`, `bitwarden-generators`, `bitwarden-api-api`, `bitwarden-vault`, and `bitwarden-crypto-sync-handler` Rust crates, together with their transitive SDK dependencies, from the Bitwarden `sdk-internal` repository, pinned to commit `56c7617b23501af70221094cdee59e10b2a9d8d2`. The SDK source offers GPL-3.0 or the Bitwarden SDK License; this project uses GPL-3.0-only. Tests use the SDK's public synthetic account fixtures and associated encrypted fixture keys; these are not production account data. See [`LICENSE`](LICENSE) and the pinned upstream [license file](https://github.com/bitwarden/sdk-internal/blob/56c7617b23501af70221094cdee59e10b2a9d8d2/LICENSE_GPL.txt).

## EFF long passphrase word list

The SDK's passphrase generator uses the EFF long word list. The list is authored and maintained by the Electronic Frontier Foundation and is available from [EFF Dice-Generated Passphrases](https://www.eff.org/dice). This application consumes the list through the SDK and does not maintain a separate copy.
