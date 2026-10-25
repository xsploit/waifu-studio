# Emotion observer hook

Requires Python 3 and host support for the `emotion.changed` one-shot hook.
In Extensions, choose **Add local integration…** and select this folder.
New installs are disabled: grant **Observe reply and speech activity**
(`lifecycle.observe`), then choose **Enable hooks**. Local code runs without
a sandbox. It launches only when a matching event arrives, records that event,
and exits; it does not keep a background process running.

The integration data folder's `latest.json` is replaced on each invocation
(at most 1 KiB). It contains only `type`, the original source `character` UUID,
`owner` (`studio` or `scene_chat`), `previous`, `emotion`, and nullable `reply_id`.
An unrelated loaded avatar or `target.changed` event does not replace the
source character. The receipt supplies no action target and sends no actions.

This example requests no reply text, credentials, provider keys, network access,
expressions, or motions. It contains no animations. A receipt shows observed
source metadata; it is not proof of a visible avatar expression or motion.
Disabling stops hook handling. Removal unregisters the integration; its installed
files and data are retained. See ATTRIBUTION.md and LICENSE for MIT attribution.
