# Windows and Linux

Waifu Studio uses the same Rust/Bevy application on Windows and Linux. Platform adapters supply desktop geometry, global shortcuts, local time, and child-process ownership. This document separates native support from live-service and hardware acceptance.

## Launch from source

Install Rust using [rustup](https://rust-lang.org/tools/install/). The repository pins Rust 1.95.0; vendored VRM code uses stable APIs compatible with that version.

Windows 10/11 x64 also needs Visual Studio's **Desktop development with C++** workload and Windows SDK. Run `run.cmd`, or `run.cmd --studio`, from any directory. `run.ps1` prepends the rustup directory so an older standalone Rust installation does not silently supply a different compiler. The first run builds an optimized executable into `target/windows/release/`.

Linux needs the [Bevy system libraries](https://bevy.org/learn/quick-start/getting-started/setup/). On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config libasound2-dev libudev-dev \
  libx11-dev libxkbcommon-dev libwayland-dev libxrandr-dev libxi-dev \
  libxcursor-dev libxinerama-dev
./run.sh
# Full studio:
./run.sh --studio
```

Use the GPU driver's Vulkan packages for your distribution. The Linux launcher builds into `target/linux/release/`. Both launchers respect `CARGO_TARGET_DIR`, `WAIFU_STUDIO_ASSET_ROOT`, and `WAIFU_STUDIO_BUNDLE`. Keep build caches separate when sharing a checkout between Windows and WSL/Linux.

The existing Linux Vulkan/X11 guard opens the opaque Studio window because that backend combination cannot reliably supply desktop alpha. Use a native Wayland session for the transparent desktop companion; KWin and Hyprland provide the advanced surface bridges.

Profiles are separate from the source and executable: normally `%LOCALAPPDATA%\waifu-studio\data` on Windows and `~/.local/share/waifu-studio` on Linux. `--data-dir PATH` chooses a different library. Existing imported absolute paths are not automatically translated between operating systems; re-import external models/motions after moving them. Bundled character receipts support relocation.

## Native adapters

| Feature | Windows | Linux |
| --- | --- | --- |
| Studio, VRM rendering, chat/provider UI, local settings | Native Bevy/winit | Native Bevy/winit |
| Desktop surfaces and taskbar seats | Win32/DWM visible window bounds and actual bottom taskbars; minimized/cloaked windows excluded | Existing KWin and Hyprland bridges; other desktops retain basic overlay controls |
| Coordinates | One companion DPI scale for all physical desktop coordinates, including negative monitor positions | Existing compositor logical coordinates |
| Global voice shortcuts | Default-on `RegisterHotKey`; conflicts are reported per binding and registrations are released on disable/exit | Existing desktop GlobalShortcuts portal |
| Extension and coding-agent cleanup | Child starts suspended, joins a kill-on-close Job Object, then resumes; bounded pipe transport | Existing process groups, nonblocking pipes, Linux parent-death handling |
| Discord Identify admission | Shared file lock, persisted uptime plus wall-clock checks | Existing boot ID, boot clock and shared file lock |
| Routine local time | Windows local time including OS timezone/DST | libc local time including OS timezone/DST |

Global shortcuts use Ctrl+Shift+Space (microphone), Ctrl+Shift+M (voice replies), Ctrl+Shift+. (stop), and Ctrl+Shift+, (voice settings). They are enabled by default for new profiles and can be disabled in Voice settings. Linux may let the desktop choose alternate bindings; Windows uses these fixed bindings and reports conflicts.

Saved credentials are read automatically from the profile's `credentials.env`. Supported names are `AI_GATEWAY_API_KEY`, `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`, `FISH_API_KEY`, `FISH_REFERENCE_ID`, and `TAVILY_API_KEY`. Explicit process environment settings retain precedence. On Linux the file must belong to the current user and have mode `0600`. The conversational tool loop is off by default; enable it and choose its tools under Assistant tools. Normal chat makes no tool requests while it is off.

Windows cannot execute Unix shebangs. For installed `.py` extensions, the host invokes `py.exe -3 -u`; install Python 3 with its launcher, or set `WAIFU_STUDIO_PYTHON` to your Python executable. For `.js`, `.mjs`, and `.cjs`, install Node.js or set `WAIFU_STUDIO_NODE`. Script paths and arguments remain separate argv entries. Native executables are launched directly. Python/Node are optional dependencies for those extension packs, not requirements for the main studio.

Preview packages include Silero 6.2.1 and ONNX Runtime 1.24.4 CPU libraries for the target platform. Blank model/runtime settings resolve bundled files; explicit overrides are retained. For source builds run `python tools/prepare_silero.py` (or `python3` on Linux); `--platform all` prepares both releases. Downloads are pinned by SHA-256, and packages retain upstream licenses and provenance. Energy VAD remains available without ONNX Runtime.

## Desktop rendering reference

[Desktop Homunculus](https://github.com/not-elm/desktop-homunculus/tree/30d11a5ad72f97f405117086839acd0ea87e1c49) supports Windows and uses the same Bevy desktop-mascot architecture. Its `homunculus_windows` and `homunculus_screen` implementations were inspected for premultiplied alpha, native window enumeration, DPI handling and the exact-monitor-size transparency issue. Waifu Studio keeps a one-physical-pixel margin at the desktop window size limit on Windows.

Homunculus documents an NVIDIA Vulkan transparency workaround: **NVIDIA Control Panel → Manage 3D Settings → Vulkan/OpenGL present method → Prefer native**. If a Vulkan desktop window is black, see its [installation guide](https://not-elm.github.io/desktop-homunculus/getting-started/installation/). This is a driver setting, not a change the application silently applies. Test your actual graphics backend; an offscreen image alone does not prove desktop transparency.

Mate Engine remains a desktop-behavior reference. No Mate implementation code or assets are included in this port.

## Build and package

```text
python tools/prepare_silero.py
cargo check --locked --all-targets
npm ci --prefix tools/discord-voice --no-audit --no-fund
cargo test --locked --all-targets -- --test-threads=1
cargo build --release --locked --bin waifu-studio
```

`.github/workflows/cross-platform.yml` runs package integrity checks, native tests, release builds and downloadable archive packaging on Windows and Ubuntu. Its hosted execution requires a push or manual workflow dispatch; adding the file does not itself establish a hosted CI pass.

The packaging tools accept an explicit built binary and copy only runtime trees and notices, never a profile directory:

```powershell
py -3 tools/package_windows.py --repo . --binary target/windows/release/waifu-studio.exe --output dist/waifu-studio-windows --archive dist/waifu-studio-windows.zip
```

```sh
python3 tools/package_linux.py --repo . --binary target/linux/release/waifu-studio --output dist/waifu-studio-linux --archive dist/waifu-studio-linux.tar.gz
```

The commands above produce local candidates. After committing all inputs, add `--release v0.1.0-preview.2` to record the release tag and source commit in an experimental preview package. See [preview setup and known issues](PREVIEW.md). The executable and `assets/` must stay together. Runtime users do not need Rust/Cargo. Linux system-library compatibility remains distribution-dependent.

## Validation

Current local test and rendering results are recorded in `VALIDATION.md`. Live provider calls, physical microphone/speaker behavior, every multi-monitor/DPI arrangement, and KDE/Hyprland desktop sessions require their own acceptance. A WSL build or software-rendered image does not establish those hardware or compositor checks.

The WSL software-Vulkan check rendered AliciaSolid. Its llvmpipe driver limits storage-buffer bindings to 128 MiB; the bundled Hikari wardrobe model exceeds that limit and needs a capable hardware GPU. The Windows GPU check rendered Hikari in Studio and transparent Desktop modes.
