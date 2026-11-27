# Bytewarden GPUI Windows idle patch

This is a repository-owned copy of the published `gpui` 0.2.2 package used only by `apps/Bytewarden.Gpui` via that app's `[patch.crates-io]` entry. The package was copied from Cargo's normalized registry source (184 files; 7,957,076 bytes) and retains upstream license and build assets. The adjacent `.cargo-checksum.json` is upstream registry provenance, not a checksum of the local modifications.

Bytewarden-specific changes are limited to:

- `src/platform/windows/platform.rs`: skip display-rate waits when every app window is hidden/minimized; poll at 100 ms so new visibility and device-loss checks remain bounded. Continue the normal VSync cadence for visible inactive/occluded windows, and only invalidate windows that can draw.
- `src/platform/windows/events.rs`: suppress non-forced rendering while hidden/minimized, explicitly validate paint regions without a frame callback, and preserve the saved frame callback across repeated minimize messages.

The 100 ms hidden poll is an explicit latency/power trade-off: a tray restore may wait up to one poll interval for the worker's next invalidation (native show/size events remain immediate). GPU device-loss recovery is still checked while hidden and forced recovery draws remain permitted. This patch does not claim zero CPU, display-present timing, or long-term memory stability.

Validation must include the complete GPUI test suite and strict Clippy, then owned-process matched visible / minimized / tray-hidden / restored idle samples, UI restore/resize/animation checks, and GPU/display recovery checks. Do not use stale process metadata or perturb user-owned windows when sampling.
