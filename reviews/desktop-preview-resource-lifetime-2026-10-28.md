# Hidden preview resource lifetime

The settings preview camera is inactive in desktop mode and outside the Studio
Characters and Scene tabs. Previously its full-size render target and CPU pixel
vector stayed allocated through the strong egui texture and camera handles.
The target now shrinks to 1x1 and replaces its CPU backing when hidden; on
activation the same image handle and egui texture ID grow to the requested
preview size. Repeated inactive frames do not rewrite the camera or resize the
asset. Orbit, zoom and framing state are retained.

At the default 600x900 RGBA8 target, the original CPU pixel vector contains
2,160,000 bytes; the inactive replacement contains four. Allocator reuse,
render-world lifetimes, driver allocation and actual process RSS or VRAM
reduction have not been measured. This is a small resource-lifetime fix, not a
demonstrated solution to the Windows CPU problem.

All four preview tests pass in Windows and Linux release builds, covering
desktop and Studio tab changes, target dimensions, old CPU buffer capacity,
camera activity, texture identity and preview controls. A Windows windowless
GPU showcase rendered 30 frames across Scene, Chat and Scene using an isolated
profile. Frames 9 and 20/29 visibly show the Hikari preview before and after
returning. This checks tab restoration, not a real desktop-mode benchmark or
real user-profile state. The running application was not replaced or restarted.
