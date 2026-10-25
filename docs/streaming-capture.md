# Local streaming capture

## Current Linux support

Use the native Wayland backend for a transparent Desktop companion. On the tested
KDE/Wayland session, `waifu-studio --desktop --capture FILE.png` loads Hikari,
saves an RGBA frame with transparent background and exits normally. This is a
native-window frame check, not an OBS recording or audio-routing acceptance test.

Vulkan/X11 (including XWayland) Desktop transparency is unavailable in this build.
The tested surface supports only opaque alpha; requesting premultiplied alpha
previously crashed before model load. The app now uses automatic surface alpha
on that actual backend and returns to Studio with an explanation. Normal Studio
remains usable. Wayland and other graphics backends are unchanged by this guard.
No compositor or global display settings are changed.

## Prepare an OBS check

1. Start Waifu Studio normally in your Wayland session. It opens in Desktop mode;
   load the intended character through the right-click menu. Separate recovery
   controls are needed only if you explicitly enable click-through.
2. In an isolated OBS scene, add a PipeWire window/screen capture source. In the
   desktop sharing dialog, select only the **Waifu Studio** companion window,
   rather than the monitor or the separate **Desktop controls** window.
   The user must make/approve this desktop-sharing selection.
3. Place a contrasting scene background behind the source. Check whether the
   captured background is transparent and whether avatar edges look correct.
   A transparent PNG alone cannot prove this capture-source behavior.
4. Configure the intended local audio source without monitoring it back into
   the microphone. Record a short spoken reply and confirm lip sync, reaction
   → talking → idle, stop/cancel and audio routing in the resulting file.
5. Repeat after resizing, opening/closing settings and restarting the source.
   Save the recording and capture-source settings as acceptance evidence.

Do not claim this checklist has passed until a real recording has been inspected.
No Twitch account or live broadcast is needed for a local test. Changing the
capture source, encoder or display backend can change its behavior.

An attempted FFmpeg `x11grab -window_id` recording of only the isolated XWayland
app produced zero frames with XGetImage BadMatch/permission errors on this host.
FFmpeg returned status0 anyway. Count/decode actual video frames before accepting
such a recording; an output filename or zero exit status is insufficient. This
failed attempt did not record the monitor or other applications.
