**Changes requested.** I found four remaining P2 issues by inspecting the current working tree. These are source-level findings; I did not reproduce them in the running application.

1. **[P2] Clothing above the hips can still widen the body proxy** — [animation_body.rs](/D:/WaifuStudio-clipfix/src/animation_body.rs:523)  
   Every vertex above the hip joint with sufficient direct torso-bone weight is accepted, including a jacket, belt, or skirt waistband weighted to those bones. If enough garment vertices fall in a height band, they can widen the fitted ellipse and push a hand that is already clear of the body. The revised filter only excludes certain garment vertices *below* the hips. Add a clothed-mesh fixture and a selection strategy that distinguishes those surfaces, or narrow the feature’s stated guarantee.

2. **[P2] Capture can permanently leave one clearance mode without usable points** — [animation_body.rs](/D:/WaifuStudio-clipfix/src/animation_body.rs:91)  
   Capture stops as soon as *either* point collection is nonempty. For example, a mesh with points accepted by the old guard but no strict points will prevent later capture attempts, so soft mode remains ineffective even if its mesh data becomes available. The converse can affect the old guard. Track readiness and retry separately for each consumer, including partially loaded mesh assets.

3. **[P2] Stopping and restarting motion can restore a stale correction** — [animation.rs](/D:/WaifuStudio-clipfix/src/animation.rs:1714)  
   With soft clearance active, stop the motion and then start another on the same avatar. The stop path restores the idle pose but leaves `soft_arms` unchanged; the next clip starts smoothing from the old correction even though it was no longer visible. That can cause an abrupt arm movement. Reset the soft state when restoring idle, and test stop followed by another clip.

4. **[P2] The proxy ignores intersections near its top and bottom** — [animation_soft_clearance.rs](/D:/WaifuStudio-clipfix/src/animation_soft_clearance.rs:125)  
   The first and last bands sit *inside* the sampled body’s height range, yet `intrusion` returns clear whenever a sample’s center is outside those band centers. A hand can therefore overlap the upper or lower body while its center falls just beyond a band; its radius is also ignored at that boundary. Extend the end bands to the sampled bounds and account for sphere overlap across the ends.

I reran the corrected metric script on the saved walk captures: it reports **5.48 cm** maximum hand displacement and **1.12 cm** maximum per-frame correction change. The intrusion figures remain proxy measurements, and the log contains 64 reports for 61 captured frames, so those counts should not be treated as a one-to-one frame comparison. The saved captures also do not establish behavior for the edge cases above.

`git diff --check` passed. I did not run Cargo tests or perform live visual acceptance; the working tree was left unchanged.