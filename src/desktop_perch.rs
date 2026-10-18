//! Portable placement math in top-left-origin logical pixels.
//!
//! These are screen/manual anchors, not external application window tracking.
//! The host supplies one consistent coordinate space and applies the result.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerchMode {
    #[default]
    Disabled,
    ScreenBottom,
    /// Bottom-left perch; no sideways rotation of the avatar is implied.
    ScreenLeft,
    /// Bottom-right perch; no sideways rotation of the avatar is implied.
    ScreenRight,
    ManualSurface,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualSurface {
    /// Left/top of a horizontal seat line, normalized within the viewport.
    pub x_fraction: f32,
    pub y_fraction: f32,
    pub width_fraction: f32,
}
impl Default for ManualSurface {
    fn default() -> Self {
        Self {
            x_fraction: 0.15,
            y_fraction: 0.72,
            width_fraction: 0.70,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerchSettings {
    pub mode: PerchMode,
    /// Seat position across the chosen screen edge or manual surface.
    pub horizontal_fraction: f32,
    /// Logical pixel padding between the avatar rectangle and screen boundary.
    pub edge_margin: f32,
    pub manual_surface: ManualSurface,
    /// Distance from the avatar rectangle bottom to its seated hips anchor.
    /// Host should calibrate this to its seated pose / camera framing.
    pub feet_inset: f32,
}
impl Default for PerchSettings {
    fn default() -> Self {
        Self {
            mode: PerchMode::Disabled,
            horizontal_fraction: 0.80,
            edge_margin: 12.,
            manual_surface: ManualSurface::default(),
            feet_inset: 54.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportRect {
    /// Global logical coordinates; negative origins support secondary monitors.
    pub origin: [f32; 2],
    pub size: [f32; 2],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerchPlacement {
    pub window_origin: [f32; 2],
    /// Actual effective size; oversized input is fitted uniformly to the viewport.
    pub avatar_size: [f32; 2],
    /// Bottom-center of the fitted avatar rectangle, not a tracked skeleton joint.
    pub feet: [f32; 2],
    /// Effective hip/seat line after viewport clamping, rather than an OS window edge.
    pub surface_y: f32,
}
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}
impl PerchSettings {
    pub fn sanitized(self) -> Self {
        Self {
            horizontal_fraction: finite_or(self.horizontal_fraction, 0.8).clamp(0., 1.),
            edge_margin: finite_or(self.edge_margin, 12.).clamp(0., 512.),
            feet_inset: finite_or(self.feet_inset, 54.).clamp(0., 2048.),
            manual_surface: ManualSurface {
                x_fraction: finite_or(self.manual_surface.x_fraction, 0.15).clamp(0., 1.),
                y_fraction: finite_or(self.manual_surface.y_fraction, 0.72).clamp(0., 1.),
                width_fraction: finite_or(self.manual_surface.width_fraction, 0.7).clamp(0., 1.),
            },
            ..self
        }
    }
}

/// Calculates a bounded avatar rectangle. No I/O, window enumeration or OS positioning.
/// Returns None for disabled mode, invalid rectangles, or unrepresentable coordinates.
/// Host must use `avatar_size` as well as `window_origin` if it permits oversized input.
pub fn calculate_placement(
    settings: &PerchSettings,
    viewport: ViewportRect,
    avatar_size: [f32; 2],
) -> Option<PerchPlacement> {
    if settings.mode == PerchMode::Disabled
        || viewport
            .origin
            .iter()
            .chain(viewport.size.iter())
            .chain(avatar_size.iter())
            .any(|v| !v.is_finite())
        || viewport
            .size
            .iter()
            .chain(avatar_size.iter())
            .any(|v| *v <= 0.)
    {
        return None;
    }
    let s = settings.sanitized();
    let [vx, vy] = viewport.origin;
    let [vw, vh] = viewport.size;
    if !(vx + vw).is_finite() || !(vy + vh).is_finite() {
        return None;
    }
    // Retain at least half the viewport even for excessive persisted margins.
    let margin = s.edge_margin.min(vw * 0.25).min(vh * 0.25);
    let available = [vw - 2. * margin, vh - 2. * margin];
    let fit = (available[0] / avatar_size[0])
        .min(available[1] / avatar_size[1])
        .min(1.);
    let size = [avatar_size[0] * fit, avatar_size[1] * fit];
    if size.iter().any(|v| !v.is_finite() || *v <= 0.) {
        return None;
    }
    let inset = (s.feet_inset * fit).min(size[1]);
    let left = vx + margin;
    let top = vy + margin;
    let right = vx + vw - margin - size[0];
    let bottom = vy + vh - margin - size[1];
    // Extreme f32 coordinate cancellation cannot produce reversed clamp bounds.
    if right < left || bottom < top {
        return None;
    }
    let (wanted_x, wanted_y) = match s.mode {
        PerchMode::ManualSurface => {
            let surface = s.manual_surface;
            let start = vx + surface.x_fraction * vw;
            let width = surface.width_fraction.min(1. - surface.x_fraction) * vw;
            let center = start + width * s.horizontal_fraction;
            (
                center - size[0] * 0.5,
                vy + surface.y_fraction * vh - size[1] + inset,
            )
        }
        PerchMode::ScreenLeft => (left, bottom),
        PerchMode::ScreenRight => (right, bottom),
        PerchMode::ScreenBottom => (left + (right - left) * s.horizontal_fraction, bottom),
        PerchMode::Disabled => return None,
    };
    let x = wanted_x.clamp(left, right);
    let y = wanted_y.clamp(top, bottom);
    Some(PerchPlacement {
        window_origin: [x, y],
        avatar_size: size,
        feet: [x + size[0] * 0.5, y + size[1]],
        surface_y: y + size[1] - inset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn viewport() -> ViewportRect {
        ViewportRect {
            origin: [-1920., -200.],
            size: [1920., 1080.],
        }
    }
    #[test]
    fn disabled_and_invalid_viewports_never_move_windows() {
        assert!(calculate_placement(&PerchSettings::default(), viewport(), [200., 300.]).is_none());
        let s = PerchSettings {
            mode: PerchMode::ScreenBottom,
            ..Default::default()
        };
        for size in [[0., 10.], [f32::NAN, 10.], [10., -1.]] {
            assert!(
                calculate_placement(&s, ViewportRect { size, ..viewport() }, [200., 300.])
                    .is_none()
            );
            assert!(calculate_placement(&s, viewport(), size).is_none());
        }
    }
    #[test]
    fn screen_edges_and_negative_monitor_origin_are_exact() {
        let mut s = PerchSettings {
            mode: PerchMode::ScreenLeft,
            ..Default::default()
        };
        let left = calculate_placement(&s, viewport(), [200., 300.]).unwrap();
        assert_eq!(left.window_origin, [-1908., 568.]);
        assert_eq!(left.feet, [-1808., 868.]);
        assert_eq!(left.surface_y, 814.);
        s.mode = PerchMode::ScreenRight;
        assert_eq!(
            calculate_placement(&s, viewport(), [200., 300.])
                .unwrap()
                .window_origin,
            [-212., 568.]
        );
    }
    #[test]
    fn manual_anchor_matches_hip_line_and_clamps_at_top() {
        let mut s = PerchSettings {
            mode: PerchMode::ManualSurface,
            ..Default::default()
        };
        s.manual_surface = ManualSurface {
            x_fraction: 0.,
            y_fraction: 0.5,
            width_fraction: 1.,
        };
        s.horizontal_fraction = 0.5;
        let placement = calculate_placement(&s, viewport(), [200., 300.]).unwrap();
        assert_eq!(placement.surface_y, 340.);
        assert_eq!(placement.window_origin[0], -1060.);
        s.manual_surface.y_fraction = 0.;
        assert_eq!(
            calculate_placement(&s, viewport(), [200., 300.])
                .unwrap()
                .window_origin[1],
            -188.
        );
    }
    #[test]
    fn malformed_settings_and_oversized_avatars_stay_inside_viewport() {
        let s = PerchSettings {
            mode: PerchMode::ManualSurface,
            horizontal_fraction: f32::NAN,
            edge_margin: f32::INFINITY,
            feet_inset: -200.,
            manual_surface: ManualSurface {
                x_fraction: 8.,
                y_fraction: -4.,
                width_fraction: 10.,
            },
        };
        let placement = calculate_placement(&s, viewport(), [10000., 10000.]).unwrap();
        assert!(placement.window_origin[0] >= -1920. && placement.window_origin[1] >= -200.);
        assert!(placement.window_origin[0] + placement.avatar_size[0] <= 0.);
        assert!(placement.window_origin[1] + placement.avatar_size[1] <= 880.);
        assert_eq!(placement.avatar_size[0], placement.avatar_size[1]);
        let parsed: PerchSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, PerchSettings::default());
    }
}
