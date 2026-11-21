//! Soft arm clearance: keeps arms out of the avatar's own body without reshaping
//! gestures, for any humanoid VRM.
//!
//! Body: vertices skinned directly to humanoid torso/upper-leg bones (see
//! `MeshBodyCache::posed_strict`), so skirt, hair and coat spring chains never count.
//! Each height band is fitted with an ellipse from percentile extents, which ignores
//! seams and small accessories and keeps the waist concave between bands.
//! Correction: one small rigid rotation of the whole arm about the shoulder. Elbow
//! bend and the hand's pose relative to the arm are unchanged. The rotation is capped
//! and rate-limited, so it never jumps; it is a no-op when the arm is already clear.
use super::{PoseBone, animation_clearance::model_transform};
use bevy::prelude::*;
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName as Bone;

/// Largest correction, about 14 degrees.
pub(super) const MAX_ANGLE: f32 = 0.24;
/// Correction rate limits (radians per second). At 30 fps the attack moves a hand
/// 0.7 m from the shoulder by at most about 1.6 cm per frame.
const ATTACK: f32 = 0.7;
const RELEASE: f32 = 0.45;
const BANDS: usize = 10;
/// Extent percentile per band; trims buttons, seams and small accessories.
const PERCENTILE: f32 = 0.96;
/// Body samples kept per frame; the fit is percentile-based, so a stride is enough.
const MAX_FIT_POINTS: usize = 3_000;

/// Avatar body axes in model space, rebuilt every frame from the posed skeleton.
#[derive(Clone, Copy, Debug)]
pub(super) struct BodyFrame {
    origin: Vec3,
    up: Vec3,
    lateral: Vec3,
    forward: Vec3,
}

impl BodyFrame {
    pub(super) fn new(hips: Vec3, neck: Vec3, left_shoulder: Vec3, right_shoulder: Vec3) -> Option<Self> {
        let up = (neck - hips).try_normalize()?;
        let across = left_shoulder - right_shoulder;
        let lateral = (across - up * across.dot(up)).try_normalize()?;
        Some(Self {
            origin: hips,
            up,
            lateral,
            forward: lateral.cross(up),
        })
    }

    fn local(&self, point: Vec3) -> Vec3 {
        let d = point - self.origin;
        Vec3::new(d.dot(self.lateral), d.dot(self.up), d.dot(self.forward))
    }
}

#[derive(Clone, Copy, Debug)]
struct Band {
    height: f32,
    center: Vec2,
    half: Vec2,
}

/// Stack of horizontal ellipses, linearly interpolated between band centers so
/// the surface (and therefore the correction) is continuous in height.
pub(super) struct BodyProxy {
    frame: BodyFrame,
    bands: Vec<Band>,
    /// Sampled body height range; the end bands extend to these bounds.
    bottom: f32,
    top: f32,
}

fn spread(values: &mut [f32]) -> (f32, f32) {
    values.sort_unstable_by(f32::total_cmp);
    let last = (values.len() - 1) as f32;
    let lo = values[((1. - PERCENTILE) * last).round() as usize];
    let hi = values[(PERCENTILE * last).round() as usize];
    (lo, hi)
}

impl BodyProxy {
    pub(super) fn fit(points: &[Vec3], frame: BodyFrame) -> Option<Self> {
        let stride = points.len().div_ceil(MAX_FIT_POINTS).max(1);
        let local: Vec<Vec3> = points
            .iter()
            .step_by(stride)
            .map(|p| frame.local(*p))
            .filter(|p| p.is_finite())
            .collect();
        if local.len() < 32 {
            return None;
        }
        let lo = local.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let hi = local.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        let step = (hi - lo) / BANDS as f32;
        if !step.is_finite() || step < 1e-4 {
            return None;
        }
        let mut bands = Vec::with_capacity(BANDS);
        let (mut xs, mut zs) = (Vec::new(), Vec::new());
        for i in 0..BANDS {
            let height = lo + (i as f32 + 0.5) * step;
            xs.clear();
            zs.clear();
            for p in &local {
                if (p.y - height).abs() <= step * 0.75 {
                    xs.push(p.x);
                    zs.push(p.z);
                }
            }
            if xs.len() < 8 {
                continue;
            }
            let (x0, x1) = spread(&mut xs);
            let (z0, z1) = spread(&mut zs);
            bands.push(Band {
                height,
                center: Vec2::new(x0 + x1, z0 + z1) * 0.5,
                half: Vec2::new((x1 - x0) * 0.5, (z1 - z0) * 0.5).max(Vec2::splat(1e-3)),
            });
        }
        (bands.len() >= 2).then_some(Self {
            frame,
            bands,
            bottom: lo,
            top: hi,
        })
    }

    /// How far a sphere at `point` with `radius` intrudes into the body, and the
    /// outward surface normal there. None when it is clear or above/below the body.
    pub(super) fn intrusion(&self, point: Vec3, radius: f32) -> Option<(f32, Vec3)> {
        let l = self.frame.local(point);
        let (first, last) = (self.bands.first()?, self.bands.last()?);
        // A sphere just beyond the sampled top or bottom still overlaps by the
        // part of its radius that reaches into the body.
        let gap = (self.bottom - l.y).max(l.y - self.top).max(0.);
        if !l.is_finite() || gap >= radius {
            return None;
        }
        // Between the sampled bounds and the outermost band centers, the end band applies.
        let y = l.y.clamp(first.height, last.height);
        let i = self
            .bands
            .partition_point(|band| band.height <= y)
            .clamp(1, self.bands.len() - 1);
        let (a, b) = (self.bands[i - 1], self.bands[i]);
        let t = ((y - a.height) / (b.height - a.height)).clamp(0., 1.);
        let center = a.center.lerp(b.center, t);
        let half = a.half.lerp(b.half, t);
        let q = Vec2::new(l.x, l.z) - center;
        let distance = q.length();
        let scaled = Vec2::new(q.x / half.x, q.y / half.y).length();
        // Boundary distance along the ray from the band center through the point.
        let (boundary, direction) = if distance > 1e-6 && scaled > 1e-9 {
            (distance / scaled, q / distance)
        } else {
            (half.x, Vec2::X)
        };
        let depth = boundary - distance + radius - gap;
        if depth <= 0. {
            return None;
        }
        let gradient = Vec2::new(q.x / (half.x * half.x), q.y / (half.y * half.y));
        let n = gradient.try_normalize().unwrap_or(direction);
        Some((depth, self.frame.lateral * n.x + self.frame.forward * n.y))
    }
}

/// Smallest rotation (scaled axis, model space) about `shoulder` that moves every
/// `(point, radius)` sample out of the body, capped at `max_angle`. Zero when clear.
pub(super) fn required_rotation(
    shoulder: Vec3,
    samples: &[(Vec3, f32)],
    proxy: &BodyProxy,
    max_angle: f32,
) -> Vec3 {
    let mut rotation = Quat::IDENTITY;
    for _ in 0..6 {
        let worst = samples
            .iter()
            .filter_map(|(point, radius)| {
                let moved = shoulder + rotation * (*point - shoulder);
                proxy
                    .intrusion(moved, *radius)
                    .map(|(depth, normal)| (depth, normal, moved - shoulder))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let Some((depth, normal, arm)) = worst else {
            break;
        };
        // Rotating about arm x normal moves the sample along the normal at
        // |arm x normal| per radian; keep a floor so near-parallel arms stay bounded.
        let axis = arm.cross(normal);
        let Some(direction) = axis.try_normalize() else {
            break;
        };
        let lever = axis.length().max(arm.length() * 0.3);
        if lever < 1e-5 {
            break;
        }
        let next = (Quat::from_axis_angle(direction, depth / lever) * rotation).normalize();
        let scaled = next.to_scaled_axis();
        if scaled.length() >= max_angle {
            return scaled.clamp_length_max(max_angle);
        }
        rotation = next;
    }
    rotation.to_scaled_axis()
}

/// Move the applied correction toward the target at bounded angular speed.
pub(super) fn approach(current: Vec3, target: Vec3, delta_seconds: f32) -> Vec3 {
    let rate = if target.length() > current.length() {
        ATTACK
    } else {
        RELEASE
    };
    current + (target - current).clamp_length_max(rate * delta_seconds.clamp(0., 0.1))
}

/// Per-avatar correction state: the rotation currently applied to each arm.
#[derive(Default)]
pub(super) struct SoftArmState {
    root: Option<Entity>,
    applied: [Vec3; 2],
}

impl SoftArmState {
    /// A visible correction remains (e.g. still fading out after being disabled).
    pub(super) fn active(&self) -> bool {
        self.applied.iter().any(|a| a.length_squared() > 1e-10)
    }

    /// The pose no longer shows any correction (fresh rest pose, mode switch).
    pub(super) fn reset(&mut self) {
        self.applied = [Vec3::ZERO; 2];
    }

    /// Advance one arm toward `target` (zero when disabled, so it fades out
    /// smoothly). Returns the rotation to show and whether it is still settling.
    fn advance(&mut self, side: usize, target: Vec3, enabled: bool, delta_seconds: f32) -> (Vec3, bool) {
        let target = if enabled { target } else { Vec3::ZERO };
        let next = approach(self.applied[side], target, delta_seconds);
        self.applied[side] = if next.length_squared() < 1e-12 { Vec3::ZERO } else { next };
        (self.applied[side], next.distance_squared(target) > 1e-8)
    }
}

/// Worst intrusion before and after correction, and the applied angle, per arm.
#[derive(Default, Debug, Clone, Copy)]
pub(super) struct SoftArmReport {
    pub(super) before: [f32; 2],
    pub(super) after: [f32; 2],
    pub(super) angle: [f32; 2],
    /// A correction has not reached its target yet; keep updating even if the
    /// clip is paused so the pose does not freeze mid-transition.
    pub(super) settling: bool,
}

fn worst_depth(shoulder: Vec3, samples: &[(Vec3, f32)], proxy: &BodyProxy, rotation: Quat) -> f32 {
    samples
        .iter()
        .filter_map(|(point, radius)| {
            proxy
                .intrusion(shoulder + rotation * (*point - shoulder), *radius)
                .map(|(depth, _)| depth)
        })
        .fold(0., f32::max)
}

/// Apply soft clearance to both arms. `body` is the posed strict body point set,
/// `forearm_radius` the measured sleeve radius. Clips are re-sampled every frame,
/// so the rotation is applied on top of a fresh pose and never accumulates.
#[allow(clippy::too_many_arguments)]
pub(super) fn correct_arms_soft(
    root: Entity,
    bones: &[PoseBone],
    parents: &Query<&ChildOf>,
    transforms: &mut Query<&mut Transform>,
    body: &[Vec3],
    forearm_radius: f32,
    state: &mut SoftArmState,
    enabled: bool,
    delta_seconds: f32,
) -> Option<SoftArmReport> {
    if state.root != Some(root) {
        *state = SoftArmState {
            root: Some(root),
            ..default()
        };
    }
    let find = |name| bones.iter().find(|bone| bone.bone == name);
    let position = |name, transforms: &Query<&mut Transform>| {
        let bone = find(name)?;
        model_transform(bone.entity, root, parents, transforms).map(|m| Vec3::from(m.translation))
    };
    let frame = BodyFrame::new(
        position(Bone::Hips, transforms)?,
        position(Bone::Neck, transforms)?,
        position(Bone::LeftUpperArm, transforms)?,
        position(Bone::RightUpperArm, transforms)?,
    )?;
    let proxy = BodyProxy::fit(body, frame)?;
    let radius = forearm_radius.max(0.005);
    let mut report = SoftArmReport::default();
    for (side, (upper, lower, hand)) in [
        (Bone::LeftUpperArm, Bone::LeftLowerArm, Bone::LeftHand),
        (Bone::RightUpperArm, Bone::RightLowerArm, Bone::RightHand),
    ]
    .into_iter()
    .enumerate()
    {
        let (Some(upper), Some(elbow), Some(wrist)) =
            (find(upper), position(lower, transforms), position(hand, transforms))
        else {
            continue;
        };
        let Some(shoulder_model) = model_transform(upper.entity, root, parents, transforms) else {
            continue;
        };
        let (_, upper_rotation, shoulder) = shoulder_model.to_scale_rotation_translation();
        // Upper-arm samples are omitted: at the armhole the sleeve is attached to
        // the torso by design. Elbow, forearm, wrist and palm are what clip.
        let palm = wrist + (wrist - elbow).normalize_or_zero() * radius * 2.;
        let samples = [
            (elbow, radius),
            (elbow.lerp(wrist, 0.33), radius),
            (elbow.lerp(wrist, 0.66), radius),
            (wrist, radius * 0.8),
            (palm, radius * 0.6),
        ];
        let target = required_rotation(shoulder, &samples, &proxy, MAX_ANGLE);
        let (applied, settling) = state.advance(side, target, enabled, delta_seconds);
        report.settling |= settling;
        report.before[side] = worst_depth(shoulder, &samples, &proxy, Quat::IDENTITY);
        let rotation = Quat::from_scaled_axis(applied);
        report.after[side] = worst_depth(shoulder, &samples, &proxy, rotation);
        report.angle[side] = applied.length();
        if applied.length_squared() < 1e-10 {
            continue;
        }
        // Rotate the upper arm in model space about its own origin (the shoulder);
        // the forearm and hand keep their local rotations, so the gesture is intact.
        let Some(parent) = parents.get(upper.entity).ok().map(ChildOf::parent) else {
            continue;
        };
        let Some(parent_model) = model_transform(parent, root, parents, transforms) else {
            continue;
        };
        let (_, parent_rotation, _) = parent_model.to_scale_rotation_translation();
        let local = (parent_rotation.inverse() * rotation * upper_rotation).normalize();
        if local.is_finite()
            && let Ok(mut transform) = transforms.get_mut(upper.entity)
        {
            transform.rotation = local;
        }
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic torso: elliptical cross-sections, 0.16 m wide (half) and
    /// 0.10 m deep, from hips (y = 0) to neck (y = 0.5), at the origin.
    fn torso() -> (Vec<Vec3>, BodyFrame) {
        let mut points = Vec::new();
        for i in 0..=40 {
            let y = -0.15 + i as f32 * 0.65 / 40.;
            for j in 0..48 {
                let a = j as f32 / 48. * std::f32::consts::TAU;
                points.push(Vec3::new(0.16 * a.cos(), y, 0.10 * a.sin()));
            }
        }
        let frame = BodyFrame::new(
            Vec3::ZERO,
            Vec3::new(0., 0.5, 0.),
            Vec3::new(0.2, 0.45, 0.),
            Vec3::new(-0.2, 0.45, 0.),
        )
        .unwrap();
        (points, frame)
    }

    #[test]
    fn soft_clearance_proxy_matches_body_surface() {
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        // Clearly outside the side of the body: clear.
        assert!(proxy.intrusion(Vec3::new(0.25, 0.2, 0.), 0.02).is_none());
        // Inside the side: intrudes, normal points outward along +lateral.
        let (depth, normal) = proxy.intrusion(Vec3::new(0.12, 0.2, 0.), 0.02).unwrap();
        assert!((depth - 0.06).abs() < 0.02, "depth {depth}");
        assert!(normal.x > 0.95, "normal {normal}");
        // Above the neck band: never intrudes.
        assert!(proxy.intrusion(Vec3::new(0., 0.9, 0.), 0.02).is_none());
    }

    #[test]
    fn soft_clearance_proxy_covers_the_ends_of_the_sampled_body() {
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        // Inside the body but beyond the top band's center height.
        assert!(proxy.intrusion(Vec3::new(0.12, 0.49, 0.), 0.02).is_some());
        // Center just above the sampled top (0.5), radius reaching into it: overlaps less.
        let (inside, _) = proxy.intrusion(Vec3::new(0.12, 0.49, 0.), 0.03).unwrap();
        let (beyond, _) = proxy.intrusion(Vec3::new(0.12, 0.52, 0.), 0.03).unwrap();
        assert!(beyond < inside && beyond > 0.);
        // Far enough beyond that the sphere cannot reach: clear.
        assert!(proxy.intrusion(Vec3::new(0.12, 0.56, 0.), 0.03).is_none());
        // Below the sampled bottom (-0.15), same rule.
        assert!(proxy.intrusion(Vec3::new(0.12, -0.16, 0.), 0.03).is_some());
        assert!(proxy.intrusion(Vec3::new(0.12, -0.2, 0.), 0.03).is_none());
    }

    #[test]
    fn soft_clearance_ignores_flared_garment_outside_strict_points() {
        // The strict body set never contains skirt vertices, so a hand next to
        // the hip but well inside where a skirt hem would be stays uncorrected.
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        assert!(proxy.intrusion(Vec3::new(0.22, -0.1, 0.), 0.03).is_none());
    }

    #[test]
    fn soft_clearance_clear_arm_needs_no_rotation() {
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        let shoulder = Vec3::new(0.2, 0.45, 0.);
        let samples = [(Vec3::new(0.3, 0.2, 0.), 0.03), (Vec3::new(0.35, 0.0, 0.), 0.03)];
        assert_eq!(required_rotation(shoulder, &samples, &proxy, MAX_ANGLE), Vec3::ZERO);
    }

    #[test]
    fn soft_clearance_moves_intruding_hand_out_with_small_rigid_swing() {
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        let shoulder = Vec3::new(0.2, 0.45, 0.);
        let elbow = Vec3::new(0.19, 0.2, 0.);
        let wrist = Vec3::new(0.13, 0.0, 0.02); // forearm swings into the hip
        let samples = [(elbow, 0.03), (elbow.lerp(wrist, 0.5), 0.03), (wrist, 0.025)];
        let rotation = required_rotation(shoulder, &samples, &proxy, MAX_ANGLE);
        assert!(rotation.length() > 0. && rotation.length() <= MAX_ANGLE + 1e-6);
        let q = Quat::from_scaled_axis(rotation);
        assert!(worst_depth(shoulder, &samples, &proxy, q) < 0.004);
        // Rigid swing: the elbow angle is unchanged.
        let (e, w) = (shoulder + q * (elbow - shoulder), shoulder + q * (wrist - shoulder));
        let before = (shoulder - elbow).angle_between(wrist - elbow);
        let after = (shoulder - e).angle_between(w - e);
        assert!((before - after).abs() < 1e-4);
        // And the hand moves outward (+x), not up through the shoulder.
        assert!(w.x > wrist.x);
    }

    #[test]
    fn soft_clearance_correction_is_capped() {
        let (points, frame) = torso();
        let proxy = BodyProxy::fit(&points, frame).unwrap();
        let shoulder = Vec3::new(0.2, 0.45, 0.);
        // Hand buried at the body's center: impossible to clear within the cap.
        let samples = [(Vec3::new(0.0, 0.1, 0.0), 0.03)];
        let rotation = required_rotation(shoulder, &samples, &proxy, MAX_ANGLE);
        assert!((rotation.length() - MAX_ANGLE).abs() < 1e-4);
    }

    #[test]
    fn soft_clearance_rate_limit_bounds_per_frame_change() {
        let target = Vec3::new(0., 0., MAX_ANGLE);
        let dt = 1. / 30.;
        let mut applied = Vec3::ZERO;
        for _ in 0..60 {
            let next = approach(applied, target, dt);
            assert!(next.distance(applied) <= ATTACK * dt + 1e-6);
            applied = next;
        }
        assert!(applied.distance(target) < 1e-5);
        let released = approach(applied, Vec3::ZERO, dt);
        assert!(applied.distance(released) <= RELEASE * dt + 1e-6);
    }

    #[test]
    fn soft_clearance_disable_fades_out_and_reenable_starts_from_shown_pose() {
        let mut state = SoftArmState::default();
        let target = Vec3::new(0., 0., MAX_ANGLE);
        let dt = 1. / 30.;
        for _ in 0..30 {
            state.advance(0, target, true, dt);
        }
        let built = state.applied[0];
        assert!(built.length() > 0.2);
        // Disabled: fades at the release rate, never jumps to zero.
        let (shown, settling) = state.advance(0, target, false, dt);
        assert!(settling && built.distance(shown) <= RELEASE * dt + 1e-6);
        while state.active() {
            state.advance(0, target, false, dt);
        }
        // Re-enabled after fading out: starts from zero, rate-limited again.
        let (shown, _) = state.advance(0, target, true, dt);
        assert!(shown.length() <= ATTACK * dt + 1e-6);
        // A reset (pose without correction) also restarts from zero.
        state.reset();
        assert!(!state.active());
    }

    #[test]
    fn soft_clearance_proxy_rejects_degenerate_input() {
        let frame = torso().1;
        assert!(BodyProxy::fit(&[Vec3::ZERO; 10], frame).is_none());
        assert!(BodyProxy::fit(&vec![Vec3::new(0., 0.2, 0.); 200], frame).is_none());
        assert!(BodyFrame::new(Vec3::ZERO, Vec3::ZERO, Vec3::X, -Vec3::X).is_none());
    }
}
