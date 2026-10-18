# Oma reference for the native companion

Reviewed 2026-10-17, read-only. Exact locally corroborated project: [xsploit/oma](https://github.com/xsploit/oma), local checkout `/home/subsect/Work/omarchy-jarvis`, HEAD `ef75341c2c47829bc2320fc45b5a287a88c2937d`. The root README explicitly calls it a private development repository; unauthenticated GitHub API returned 404. This is not evidence that the repository is publicly available. Public projects named Omablob, Omascot and Omarchy Deskmate are distinct; none was substituted for Oma.

## Actual source findings

- **Desktop companion:** Electron transparent orb/Live2D avatar, native voice UI, push-to-talk and other selectable listening modes, hover controls, always-on-top and pause-when-hidden preferences. Window-shape code handles the input/drawing region. This is not a Bevy/VRM implementation.
- **Architecture:** Python launcher/service in `omarchy-voice/`; Electron native entry in `chatgpt-web-server/probe-native-codex.cjs`; settings, recovery and desktop-tool modules are separate. Root README explicitly describes host-specific paths and says it is not a portable installer.
- **Voice/models:** native entry uses a locally installed Codex app-server experimental realtime/WebRTC interface. Source requests realtime model `gpt-live-1-codex`, selectable named voices, configurable Codex execution model/reasoning/search/cwd, input/output devices and audio-processing preferences. These are source strings, not a verified current public model/API offering or successful live connection. No authenticated service was contacted.
- **Personality/memory:** preferences provide response style plus a bounded custom-instructions field. Native thread creation is `ephemeral:true`; realtime instructions explicitly prohibit persistent memory and transcript saving. Oma therefore supplies no evidence for durable companion memory, daily-life journals or relationship-state parity. Do not confuse settings persistence with autobiographical memory.
- **Tools/agents:** desktop schemas and a worker support computer actions; the existing Jev bridge at `/home/subsect/Work/jev-selfbot/src/oma.mjs` imports this checkout's `native-desktop-tools.cjs`. That bridge exposes selected desktop operations, validates schemas/argument sizes, excludes general shell/terminal-execution tools, and bounds timeout/output. This establishes a real integration seam, not universal cross-platform compatibility.
- **Access:** current native settings include full/read-only access. The inspected live thread path uses `danger-full-access` plus `approvalPolicy:never` for full mode, and read-only sandbox otherwise. An approval-dialog module exists, but its existence does not mean every current live action requires an Allow-once dialog. Keep universal product permissions independently designed and explicit.
- **Recovery:** independent settings window/tray, reconnect controls and bounded recovery helpers keep controls available after voice-renderer failure. Source contains dedicated recovery/settings/approval tests; README records historical Electron preview verification. Those records were not re-run in this review and do not establish current installed-service health.

## What to carry into Waifu Studio

| Priority | Practical feature | Fit with current work |
|---|---|---|
| P0 | Always-available Settings, stop and reconnect | Keep controls independent of model/worker success; surface connection failure honestly. |
| P0 | Voice interruption and visible listening state | Extend the current voice workflow with explicit capture/output-device selection and PTT before ambient listening. |
| P1 | Lightweight desktop presence | Character-aware always-on-top, click-through regions, hover controls and idle/background frame budgets; implement natively without assuming Electron input-shape APIs transfer. |
| P1 | Content-free activity status | Show starting/running/waiting/stopping, elapsed time and last error; never fabricate percent complete. |
| P1 | Separate voice conversation and executing agent | Current bounded delegation/agent loop is the appropriate seam. Bind selected worker, cwd and access to explicit Start; quoted worker output cannot escalate permissions. |
| P2 | Narrow desktop tool adapters | Read-only observations first; explicit opt-in per capability, bounded arguments/results/timeouts, cancellable execution, separate OS backends. Host-specific Oma routes remain references rather than runtime dependencies. |
| Separate roadmap | Persistent memory, routine, moods and reflection | Use the dedicated memory/behavior design and Tamanitomo comparison; native Oma explicitly disables these. |

## License and evidence boundary

`omarchy-voice/LICENSE` grants MIT for that component. The root README preserves third-party provenance and warns that private hosting does not replace component license terms; no blanket license for all Electron code, Live2D assets or SDKs was established. Do not copy or redistribute the entire project under an assumed MIT grant. No code was copied.

Primary local sources inspected: root `README.md`; `chatgpt-web-server/native-settings.cjs`, `probe-native-codex.cjs`, `native-desktop-tools.cjs`, native module inventory, package metadata; `omarchy-voice/LICENSE`; Jev `src/oma.mjs`. Public repo checks and local source inspection establish identity and implementation structure, not installed/running state or successful current voice/desktop actions.
