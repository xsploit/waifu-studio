**Changes requested.** I found three P2 issues in the current uncommitted soft arm-clearance change. All three are established by source inspection; I did not reproduce them in the running app.

1. **[P2] An interrupted crossfade can apply the correction twice** — [animation.rs](/D:/WaifuStudio-clipfix/src/animation.rs:1740)  
   Enable soft clearance, play a pose with a visible correction, select another motion, then select a third before the 0.5-second crossfade finishes. The second selection snapshots the already-corrected visible transforms as `transition.from`; the blend uses that snapshot and then applies the stored soft correction again. Near the start of the new blend, the arm can jump outward beyond the intended rate and angle limits. Preserve an uncorrected transition source or account for the correction already in the snapshot, and test interrupted transitions.

2. **[P2] An incomplete first mesh capture can permanently disable or misfit soft clearance** — [animation_body.rs](/D:/WaifuStudio-clipfix/src/animation_body.rs:97)  
   If the first loaded skinned mesh contributes any strict points, capture reports ready and never checks for later meshes. A partial set with fewer than 32 points cannot fit a proxy, so the correction remains inactive even after the body mesh arrives; a larger partial set can instead produce an incomplete proxy and sleeve radius. Track loaded asset changes even after a nonempty capture and rebuild when the available mesh set changes; test staggered mesh loading.

3. **[P2] A paused pose never retries a failed capture** — [animation.rs](/D:/WaifuStudio-clipfix/src/animation.rs:1829)  
   Pause a motion and enable soft clearance before its mesh or bind-pose assets are available. The first attempt fails and clears the correction state; on subsequent frames the paused-pose early return prevents another capture attempt, even after the assets load. The correction appears only after another pose or control change forces an update. Keep the pose eligible for retry while requested clearance is waiting for usable assets, with a paused-loading regression test.

`git diff --check` passed. I did not run Cargo tests, render captures, or perform interactive visual acceptance; those would not be read-only checks in this environment. The prior stop/reset and proxy-end findings have corresponding source changes, but I did not independently validate those fixes at runtime. The working tree was left unchanged.

