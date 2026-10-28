# Windows repaint backport

This is `i-slint-backend-winit` 1.18.1 from crates.io, matching the application's
pinned Slint release. The original source commit is recorded in
`.cargo_vcs_info.json`. Original source files and license notices are retained.

The only implementation change is the Windows update-region fix from
[Slint PR #13568](https://github.com/slint-ui/slint/pull/13568), merged as
`08a297b` on September 28, 2026. The upstream diff applies to this release without
code changes, with offsets adjusted by Git.

Before each Windows draw, read the native `GetUpdateRect` while winit still
preserves it. Convert physical coordinates to Slint logical coordinates and
mark that region dirty. This repairs OS-discarded pixels after restore, snapping,
and display-scale changes even when no application property has changed.
Internally requested paints without a native update rectangle keep partial
rendering. Other platforms keep their original implementation.

`crates/presence-desktop/examples/repaint_probe.rs` damages an isolated native
window through GDI, invalidates it through Windows, and checks the actual
presented pixels, rather than a separately rendered Slint snapshot. It fails
against the unpatched 1.18.1 backend. Run `scripts/test-repaint.ps1` to verify
partial, bottom-right, and full invalidation at 100%, 125%, and 150% scaling.

Remove this backport when upgrading Slint to a release containing #13568. Keep
the separate `i-slint-renderer-software` typography patch until its changes are
also available upstream.
