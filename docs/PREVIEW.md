# Preview builds

This is an experimental Windows/Linux tester release. Start with Studio to configure a character and services. The setup wizard is planned and is not included yet.

Download your platform archive from [GitHub Releases](https://github.com/xsploit/waifu-studio/releases). Extract the whole archive before launching; keep the executable beside `assets/`. Repository access is required while the repository is private.

## Windows x64

Open `studio.cmd` for the full app, or `start.cmd` for the desktop companion. Rust is not required. Windows 10/11, current graphics drivers and the Microsoft Visual C++ x64 runtime are required. If Windows reports a missing `VCRUNTIME140.dll`, install the runtime from Microsoft. These preview binaries are not code-signed.

## Linux x64

The initial binary is built on Ubuntu 24.04. Run `./studio.sh` for the full app or `./start.sh` for the desktop companion. See `DEPENDENCIES.txt` for the linked libraries and symbol versions; compatibility with older distributions is not established. Install your distribution's Vulkan graphics driver and the ALSA, udev, X11/Wayland and xkbcommon runtime libraries as needed.

The X11/Vulkan path uses Studio because transparent desktop rendering is unreliable on that combination. Wayland, KDE/KWin and Hyprland behavior still needs testing on physical desktops. Software Vulkan drivers may not support the larger avatars.

## First launch

1. Choose a bundled character or import a VRM in Profiles.
2. Configure a provider and model in Connections. Supply your own API key or local model-server address, then test the connection.
3. Configure speech and microphone settings if wanted. Send a short text message before trying the microphone.
4. Save the profile and try Desktop mode. Return to Studio through the companion's right-click menu.

Profiles persist under `%LOCALAPPDATA%\waifu-studio\data` on Windows and `~/.local/share/waifu-studio` on Linux. Existing profiles are reused. No personal profile, chat history or provider credentials are shipped in the archives. Elmer, Zome and Lazuli are excluded from the preview bundle; existing local profiles are not deleted.

## Try the new controls

- **Voice & mic:** the default pause allowance is three seconds. Turn off **Stop after silence** for manual stopping (the 30-second cap still applies). Blank Silero paths use the included model/runtime.
- **Voice & hotkeys:** Ctrl+Shift+Space records, Ctrl+Shift+M toggles speech, Ctrl+Shift+period stops voice/recording, and Ctrl+Shift+comma opens voice settings. Global registration starts by default; Linux may need desktop-portal approval.
- **Chat:** drop images, PDF, DOCX, text or code into the app, then send a question. The composer shows removable attachment chips.
- **Animation:** the original 167 Kimodo-generated Waifumations clips are bundled on both platforms. Enable automatic companion motions; empty idle/talking selections use discovered categories, while saved selections must point to available files. Use Resume after manual playback.
- **Assistant tools:** the conversational agent loop is off by default. Choose Tavily search/page extraction, local read-only Codex/Claude delegation or individually enabled MCP tools, then enable the loop. Your model must support function calls; extra tool/model rounds add latency. Browser automation requires a configured Streamable HTTP browser MCP server.
- **Agent tasks:** select an installed, authenticated Codex/Claude/Pi CLI and a working directory. Start with a read-only task. Enable task proposals for the review inbox or review and start a bounded coding loop. Showing and speaking intermediate text have separate switches.

## What needs testing

- Clean-machine startup, GPU/driver compatibility, transparency, multiple monitors and mixed DPI.
- Microphone selection, speech transcription, voice playback and lip-sync during real conversations.
- Restarting and updating without losing saved character settings or conversations.
- Long sessions, desktop interactions, global shortcuts and optional integrations.

Python and Node.js are needed only for integrations that use those interpreters. Claude Code must support `--safe-mode` (verified with 2.1.310). Agent model overrides are saved separately; choose an available model when a CLI default requires extra usage credits.

The Discord voice worker is not bundled. Silero VAD and the platform-specific ONNX Runtime CPU libraries are bundled, with upstream license notices under `assets/silero/`. App/API/provider behavior, display acceptance and every integration are not covered by an automated build passing.

Report the release tag, OS, GPU, steps to reproduce and expected/actual behavior in a GitHub issue. Include a screenshot or relevant error text when helpful. Check `SHA256SUMS.txt` before installing and retain `package-manifest.json` for file integrity checks.
