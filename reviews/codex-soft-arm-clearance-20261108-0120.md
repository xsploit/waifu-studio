**Review result: changes requested.** I found five defects in the opt-in soft-clearance work. The findings are verified by source inspection or direct calculation; I did not reproduce them in the running application.

1. **[P2] Paused poses stop smoothing after one frame** — [animation.rs](/D:/WaifuStudio-clipfix/src/animation.rs:1824)  
   Pause on an intersecting pose and enable soft clearance. The first update applies one rate-limited correction, but subsequent updates return early because the pose and settings have not changed. The correction therefore never reaches its target while paused. The same problem can affect a paused final frame. Keep updating while the soft correction is still converging, and test enabling it on a paused pose.

2. **[P2] Toggling clearance can cause a sudden arm jump** — [animation_soft_clearance.rs](/D:/WaifuStudio-clipfix/src/animation_soft_clearance.rs:294)  
   Let soft clearance build up a correction, turn it off, then turn it back on for the same avatar. The off path removes the visible correction, but `SoftArmState.applied` retains the old angle. Re-enabling starts from that stale angle rather than the currently visible zero correction, bypassing the intended per-frame limit. Keep the state synchronized with the applied pose and define a smooth transition when disabling or switching modes.

3. **[P2] Garments weighted to torso bones enter the “strict body” proxy** — [animation_body.rs](/D:/WaifuStudio-clipfix/src/animation_body.rs:276)  
   A skirt or coat vertex weighted more than 90% directly to hips or another selected bone meets the same condition as a body vertex. A sufficiently represented garment can consequently widen the fitted proxy and push an otherwise clear hand away. The current garment test supplies only synthetic body points, so it does not exercise selection from a clothed mesh. Add a fixture with directly weighted garment vertices and use a body-selection strategy that handles them, or explicitly narrow the feature’s guarantee.

4. **[P2] A valid strict point set can be unusable because capture is gated on the old point set** — [animation_body.rs](/D:/WaifuStudio-clipfix/src/animation_body.rs:324)  
   Soft mode needs `strict`, but `capture()` returns success only when `points`, the old guard’s collection, is nonempty. A model can have qualifying strict upper-leg points while lacking points accepted by the old torso filter; soft correction then never runs. Conversely, once any old points exist, capture stops retrying even if the strict set is empty or incomplete. Track readiness separately for the two consumers and test both cases.

5. **[P2] The capture check understates changes in correction direction** — [metrics.py](/D:/WaifuStudio-clipfix/work/soft-clearance-eval/metrics.py:33)  
   The reported per-frame change subtracts *magnitudes* of hand displacement, rather than comparing displacement vectors. For example, corrections of 5 cm left and 5 cm right on consecutive frames report zero change, although the correction changed by 10 cm. Calculate the length of the difference between the two correction vectors, check that frame sets match, and rerun the capture assessment. The recorded 1.13 cm result does not establish that the per-frame criterion passed.

The task graph records successful prior unit and animation checks and a walk capture with reduced **proxy-measured** intrusion. Those are builder receipts, not checks I reran; the penetration metric is also based on the solver’s own proxy.

I inspected the diff, the new untracked solver, integration paths, task graph and evaluation script. `git diff --check` passed. I did not run Cargo, render the avatar, test an interactive session, or validate other VRMs. The working tree remains unchanged.