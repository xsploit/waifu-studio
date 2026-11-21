# Windows desktop-mode performance and resource lifetime

Tests used a separate local, inactive Win32 desktop and isolated profile, with the real Hikari model and 60 FPS caps. No remote connection was used. The user's running instance was left untouched. This setup is useful for repeatable mode switches and asset counts, but its window enumeration, focus and compositor behavior differ from the interactive desktop; it is not a substitute for an active-desktop benchmark. The machine was in use, so other workloads may affect the measurements. CPU values are percentages of one logical CPU, not percentages of the entire machine. Metrics and local-control instrumentation were enabled.

Follow-up correction: these original source-release runs did not set `WAIFU_STUDIO_ASSET_ROOT` and the executable had no adjacent packaged assets. The avatar, desktop idle motion, and explicitly configured photo backdrop loaded, but the bundled room artwork and motion catalog were unavailable. Thus these numbers describe that incomplete asset setup, not a fully packaged release. The room geometry and backdrop lifetime observations remain valid for the assets that did load.

## Resource lifetime

In the room test, Rose Bedroom was selected and the avatar stayed loaded. In Studio the room had one root, 51 meshes and 74 standard materials; desktop had zero room roots, 46 meshes and 52 materials. Returning to Studio restored the original counts, and a second switch reproduced them. This verifies the current source's room unloading at the Bevy asset-store level, not the release of all driver or process memory.

In the photo-backdrop test, Studio had 33 images and 10,143,660 CPU image bytes. Desktop had 32 images and 3,850,252 bytes. Both returned to the initial values on each Studio return. The saved scene choice was retained. The CPU asset counts do not measure GPU allocation or imply the operating system returns freed pages.

In one room run, private process memory increased from approximately 1,720 MiB to 2,248 MiB after the first switch back to Studio, then was approximately 2,239 and 2,261 MiB on the next desktop and Studio samples. That is a real allocation increase, not proof of a leak: graphics allocations, caching and other allocations have not been attributed. Longer switching and allocation profiling on the interactive desktop are needed.

In the final photo-backdrop run, Windows' per-process dedicated GPU memory counter reported 983.3 MiB on initial Studio, 1,164.4 MiB on desktop, 1,237.6 MiB on Studio, 1,164.4 MiB on desktop and 1,237.6 MiB on Studio. These figures stabilized after the first cycle, but desktop still used more dedicated GPU memory than the initial Studio state. The counter cannot distinguish live textures from allocator or driver retention; the source of the allocation remains unproven.

## CPU experiment

Three 12-second desktop samples with one compute worker: 29.82%, 28.77%, 33.07% of one core; measured FPS 58.15–58.82. Four-worker samples: 40.62%, 40.23%, 35.55%; FPS 58.94–59.04. Hikari's desktop idle motion was playing. Studio with a room gave 40.36%, 42.71%, 42.71% with one worker and 54.30%, 51.95%, 53.90% with four, with a momentary 55 FPS sample in the four-worker run. The runs were sequential and short and only one avatar and scene were tested. These measurements warrant interactive-desktop and heavier-scene testing, but do not establish a universally safe new default. The four-worker default remains unchanged.

## Fixes and checks

- Avoid replacing the desktop bridge's unchanged status text every frame; retain all tracking and sitting behavior. Its individual CPU impact has not been measured.
- Correct a Windows transition in which adding window decorations after the inner size request repeatedly shrank Studio. The saved size is re-applied once on the next update for non-maximized Windows windows. In an isolated Windows run, the Studio viewport returned to 2560×1800 after both switches instead of shrinking to 2534×1729 and then 2508×1658. Maximized windows continue to be restored by the window manager.
- Add CPU-side image, mesh, material and room-root counts to the explicitly enabled local-control status endpoint. These diagnostics are not a GPU-memory accounting mechanism.
- Windows release build succeeded. The window/control regression passed, and all 12 desktop-behavior tests passed. A final run of the built executable repeated the photo-backdrop cycle with stable asset counts, consistent GPU counter values on the second cycle, and preserved Studio client size. The existing animation experiments in the worktree are outside this performance review and are not visually approved by these tests.
