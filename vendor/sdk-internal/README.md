# Vendored SDK core

This crate is copied from Bitwarden sdk-internal revision
56c7617b23501af70221094cdee59e10b2a9d8d2. The local change adds an explicit
Identity-client middleware hook to ClientBuilder, because the pinned SDK otherwise
attaches custom middleware only to its API client. The WinUI client uses that hook
to carry the official newDeviceOtp field on the password-token retry.

Keep unrelated SDK source identical to the pinned revision. Re-check this patch
against upstream before changing the SDK revision.
