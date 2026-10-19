# Mate Engine efficiency source audit

Inspected local Mate Engine checkout `2c5ea6b`. Read-only source inspection; no build, benchmark or live Mate measurement. Its reported approximately 200 MB RAM is user-observed, not a matched comparison.

Workload distinction: the user reports that their Mate setup has no conversational TTS and uses simulated mouth movement. Waifu Studio additionally supports streamed synthesis, decoding/playback, audio metering and speech-driven lip sync. The quiet-avatar samples in this investigation do not exercise active speech, and must not be labeled active-TTS measurements or direct feature-equivalent Mate comparisons. Conversely, dormant speech support does not by itself explain the quiet-avatar RAM gap; the duplicate morph allocation was measured independently. No speech or lip-sync capability is removed for efficiency.

## Verified findings

- **Explicit asset ownership and cleanup:** `Assets/MATE ENGINE - Scripts/VRMLoader/VRMLoader.cs:360` destroys previous custom model objects, unloads the current asset bundle with `Unload(true)`, and clears texture references. The attached `GltfInstanceDisposer` disposes imported instances on destruction (`:540`). This is a useful model for testing repeated avatar switches, not just first-load RAM.
- **Unused-resource collection:** the loader calls `Resources.UnloadUnusedAssets` and managed GC (`VRMLoader.cs:485`); dance cleanup also unloads unused assets (`AvatarHandlers/AvatarDancePlayer.cs:938`). Our recent release of hidden rooms, background handles and CPU texture copies addresses the same resource-lifetime concern. Unity GC is not a mechanism to transplant into Rust.
- **Optional Windows working-set trimming:** `Tools/MemoryTrim.cs:63` collects managed garbage, unloads unused resources, then calls Windows `EmptyWorkingSet`. It can run after startup and every 600 seconds. It is **off by default**, including saved settings (`Settings/SaveLoadHandler.cs:170`). If enabled, it can lower resident-memory reporting without proving a corresponding reduction in committed/live memory; it is not evidence explaining the user's specific sample.
- **Adjustable frame cap:** `Settings/FPSLimiter.cs:42` sets target FPS and disables VSync. The component defaults to 60, but saved settings default to 90 (`Settings/SaveLoadHandler.cs:121`). No evidence here that Mate's default efficiency comes from a 15/30 FPS cap. Our focused/unfocused defaults are already 60/15, with higher polling while speech needs it.
- **Sleep is behavior, not rendering suspension:** `AvatarHandlers/AvatarSleepController.cs:35` continues updating and changes an Animator boolean. It does not stop rendering or cap FPS. `AvatarRebindHandler.cs:38` can explicitly select AlwaysAnimate; avoid assuming inactive-looking avatars cost nothing.
- **Quality tiers and active startup default:** `Settings/SaveLoadHandler.cs:161` defaults to index 1, applied at startup by `AvatarSettingsMenu.cs:268`. That tier has `globalTextureMipmapLimit: 1` (`ProjectSettings/QualitySettings.asset:75`), a real half-resolution texture default for textures with mipmaps, rather than merely an unused preset. This can reduce eligible GPU texture memory by about four times; it does not establish equivalent CPU RSS savings or imported-texture streaming participation. Lower tiers also reduce shadows/MSAA. The user's particular runtime setting was not measured.

## What to apply next

1. Measure repeated avatar/motion switches and verify old meshes, materials, textures and animation assets return to a stable baseline. Mate's explicit importer disposal is the strongest actionable lesson.
2. Attribute our remaining resident memory to live allocations, allocator retention and graphics-driver mappings before more changes. Compare the same avatar, viewport size, FPS and visual quality; report RSS/PSS separately from VRAM and Windows working set.
3. Quality presets could reduce usage, but are outside this task's requirement to preserve visual quality and animation cadence. First eliminate duplicate retained data and unnecessary resource/work lifetimes.

No source changes were made from this audit. It does not establish that Unity or Rust is intrinsically more efficient, or explain a 200 MB versus 652 MB difference without matched runtime evidence.
