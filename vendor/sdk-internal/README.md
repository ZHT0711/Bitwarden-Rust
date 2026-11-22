# Vendored SDK core

This crate is copied from Bitwarden sdk-internal revision
56c7617b23501af70221094cdee59e10b2a9d8d2. Local changes are limited to:

- An explicit Identity-client middleware hook on ClientBuilder, because the pinned
  SDK otherwise attaches custom middleware only to its API client. The WinUI
  client uses that hook to carry the official newDeviceOtp field on the
  password-token retry.
- Server-issued remembered two-factor token exposed in the password-login response,
  including the legacy PascalCase alias. The host keeps the credential in Rust and
  uses Windows DPAPI storage; it is never included in the UI login result.
- Wrapped password-login key data exposed to the host for local vault unlock.
- A Windows-only desktop unlock helper verifies a password against the active SDK key and
  initializes the account state from a Windows Hello `SecretProtectedKeyEnvelope`. The raw
  SDK key stays in its key store; the existing account-state validation is shared with the
  password path.
- An explicit native `with_loopback_http()` opt-in for HTTP services on localhost,
  IPv4 loopback, or IPv6 loopback. The pinned SDK otherwise forces HTTPS in
  Release builds, including loopback services. The opt-in binds Identity and API
  requests to their respective configured origins, disables proxies and automatic
  redirects, and leaves external HTTP clients and other service URLs unchanged.
  The origin check runs immediately before transport, after custom and auth
  middleware. The host still validates self-hosted URLs before constructing a
  client. Actual transport tests live in the host crate's `src/ffi/http_tests.rs`.

Keep unrelated SDK source identical to the pinned revision. Re-check these patches
against upstream before changing the SDK revision.
