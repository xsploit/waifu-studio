# Local VSync-preserving Mailbox patch

Upstream: crates.io `bevy_render` 0.19.1, MIT OR Apache-2.0. Published
sources, normalized manifest and license files are retained. The standalone
workspace boundary follows the existing local `bevy_mesh` patch.

Only `src/view/window/mod.rs` and the added sibling `presentation.rs` change
renderer behavior. Mailbox requests resolve to Mailbox when the existing
surface reports support, otherwise FIFO. Upstream's intermediate Immediate
fallback is removed for Mailbox requests. Explicit FIFO, Immediate and auto
policies are unchanged. Surface creation, ownership, acquisition, frame
latency, rendering and presentation remain upstream's implementation.

An INFO line at surface creation/reconfiguration reports requested and resolved
mode. It is not emitted per frame and does not claim physical display timing.
The original unsupported-mode fallback message remains available.

The exact capability selector is exercised by the app's
`tests/presentation_fallback.rs`, including a surface supporting Immediate and
FIFO but lacking Mailbox. No duplicate native surface is created for probing.

Upgrade procedure: diff this crate against its pinned published source;
reapply only this selection/logging change if still needed; run the fallback
tests, app runtime-policy tests, all-target tests, strict Clippy and native
FIFO/Mailbox controls. Recheck upstream's policy before carrying the patch
forward. Native timing on one adapter is not cross-platform or visual
presentation acceptance.
