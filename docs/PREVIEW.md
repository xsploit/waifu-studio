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

## What needs testing

- Clean-machine startup, GPU/driver compatibility, transparency, multiple monitors and mixed DPI.
- Microphone selection, speech transcription, voice playback and lip-sync during real conversations.
- Restarting and updating without losing saved character settings or conversations.
- Long sessions, desktop interactions, global shortcuts and optional integrations.

Python and Node.js are needed only for integrations that use those interpreters. The Discord voice worker and optional ONNX Runtime are not bundled. App/API/provider behavior, display acceptance and every integration are not covered by an automated build passing.

Report the release tag, OS, GPU, steps to reproduce and expected/actual behavior in a GitHub issue. Include a screenshot or relevant error text when helpful. Check `SHA256SUMS.txt` before installing and retain `package-manifest.json` for file integrity checks.
