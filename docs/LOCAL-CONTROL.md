# Local app control and animation review

The developer control interface is opt-in. Start the app with
`waifu-studio --control-dir PATH`. Ordinary launches do not open this interface.
It uses a session-specific local file mailbox, not a network listener, and never
moves the system mouse or types into other applications. Status includes the
active motion, measured frame cadence, focus, frame caps, gaze and face settings;
it excludes conversations, memories and provider credentials.

Use Python 3 to issue a command:

```text
python tools/studio_control.py --control-dir PATH --json "{\"action\":\"status\"}"
```

For an MCP client, configure a stdio server with command `python` and arguments
`tools/studio_control.py --control-dir PATH --mcp` (use absolute paths). The
`waifu_control` tool exposes the same bounded app commands. The running app must
have opted into that directory. This does not install an MCP server in Codex or
grant access to other applications.

Actions: `status`, `list_motions`, `play_motion` (a catalog `path`), `stop_motion`,
`set_automatic_motion` (`enabled`), `set_gaze` (`config`), `set_frame_caps`
(`focused`, `background`, `desktop`), `set_camera_view` (`view`: front, side, rear,
other-side), `restore_window`, `set_view` (studio/desktop), `set_screen`, `sit`
(`sitting`), and `capture` (a plain PNG `name`). Capture is queued; wait for the
file to exist before using it. It captures the renderer, not unrelated windows.
Settings commands save through the app's normal profile/settings paths. Use an
isolated `--data-dir` when experimenting without changing the personal profile.

Only one controller should write requests at a time. Requests are at most 64 KiB,
session-bound, and consumed once. Responses have request IDs. Captures stay in
the mailbox's `captures` directory. No arbitrary code/shell command is exposed.

## Review the complete motion library

`tools/audit_motion_channels.py` inventories every source frame and channel,
including hashes and eye tracks. It does not certify target-avatar clearance.
Body retargeting excludes both eye joints; authored gaze/face controls own eyes.

The `render_desktop_motion` example supports `--view quad` (front, side, rear,
other side) and `--batch-dir DIR`. Combine these with `--continuous --sequence 30
--time 0 --frames 60 --avatar HIKARI.vrm`. Give an output such as
`review/frames/frame.png`: each clip gets its own subdirectory, lossless PNGs and
a receipt reporting captured/source frame counts. The avatar is loaded once;
the complete source motion plays with spring physics and experimental arm
correction off. Review captures are 1280x960, containing four 640x480 views.
They are diagnostic material, not the high-resolution product video.

For locomotion, add `--follow-hips` so the cameras follow horizontal hips
translation while leaving the animation unchanged. This is recorded in each
receipt; fixed cameras can otherwise be walked through or out of frame.

Build the local review desk with `python tools/build_motion_review.py REVIEW_DIR
REVIEW_DIR/channel-audit.json`. The optional `review-findings.json` maps exact
clip filenames to `sha256`, `visual_status`, and `notes`; mismatched source
hashes are rejected. Browser review decisions can be exported separately.

The review must include the first pose, complete movement, return and loop seam;
automatic gesture/talking/idle blends need separate review. Capturing every
frame does not mean someone has visually accepted every frame. No clipping-free
library is claimed. Keep rejected/pending clips out of a demonstration playlist.
