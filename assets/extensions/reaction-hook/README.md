# Configurable reaction hook

Install this directory with **Add local integration**. Python 3 is required.
This runs local code without a sandbox. Explicitly enable hooks and grant
`lifecycle.observe`, plus `motion.play` and/or `expression.apply` for the saved
selections. No network, provider, credential or private reply-text access is used.

Save **When** (Reply finished, Speech started, Speech stopped or Emotion changed),
**Character**, **Motion**, **Expression**, and **React to events**. All four hooks
are registered, so an unselected event still starts a short child that exits
without actions. Failed and cancelled replies are ignored. A speech-stopped
event can include cancellation; it is still the selected speech-stopped trigger.

Empty Character follows the captured loaded target. A nonempty canonical UUID
only filters it: the hook never loads a character or retargets another avatar.
Normal events must name that same target UUID, even with an empty filter. Target
replacement, generation change or unload fails the invocation. The host also
checks the captured target incarnation before executing actions.

The default motion is the owned gentle nod. None selects no motion/expression;
both None completes without actions. An expression uses strength 0.75 for two
seconds. After its acceptance, the one-shot motion overlaps it through the normal
API. The child waits for both motion acceptance and the actual matching
`motion.finished` completed receipt, and retains the full expression duration.
Acceptance alone cannot complete a motion. Rejection, noncompleted motion,
invalid input, EOF while waiting or a nine-second child deadline exits with error;
the host additionally caps the process at ten seconds. Nothing is retried.

The UI's saved-reaction test starts the same handler with `--test` through normal
runtime admission. It uses saved settings and the first `target.changed`, applies
the same character filter and permissions, and does not wait for a lifecycle
event. It does not simulate a reply or prove live chat/network behavior.

Each invocation replaces a bounded fixed-state receipt in its host data folder:
`latest-hook.json` normally, `latest-test.json` for tests. `completed` means the
selected actions completed by these API criteria (or neither was selected);
`ignored` means inactive or a filter did not match; `error` means failure.
Intermediate states are `waiting`, `expression_pending`, `expression_accepted`
and `motion_pending`. No event text or settings are written. Host permission,
busy/manual ownership, catalog availability and avatar expression checks still
apply. Uninstall retains the integration data folder under the host's removal
policy. See ATTRIBUTION.md and LICENSE for the owned nod's MIT attribution.
