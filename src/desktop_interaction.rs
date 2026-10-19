//! Portable desktop drag / window-seat state machine, in top-left logical pixels.
//!
//! The host owns hit testing, pointer capture, native window enumeration and
//! positioning. A snapshot is authoritative: omit hidden/minimized/closed
//! windows, and a seat on that window immediately returns to idle.
//!
//! Adapted interaction and hips smoothing concepts from
//! https://github.com/not-elm/desktop-homunculus (homunculus_drag,
//! homunculus_sitting, homunculus_screen). This implementation adds grab offsets,
//! resize-relative anchoring, portable host snapshots and disappearing-seat cleanup.
//!
//! MIT License — Copyright (c) 2024-2026 notelm
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use bevy::prelude::{Resource, Vec2};

const MAX_COORDINATE: f32 = 16_777_216.;
pub const EDGE_THRESHOLD: f32 = 80.;
pub const HIPS_BLEND_SPEED: f32 = 4.;
pub const HIPS_SNAP_WORLD_UNITS: f32 = 0.3;
pub const SITTING_ADJUST: f32 = 0.9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub origin: Vec2,
    pub size: Vec2,
}
impl ScreenRect {
    fn valid(self) -> bool {
        valid_point(self.origin)
            && valid_point(self.size)
            && self.size.min_element() > 0.
            && valid_point(self.origin + self.size)
    }
    fn contains(self, point: Vec2) -> bool {
        point.cmpge(self.origin).all() && point.cmple(self.origin + self.size).all()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceId {
    Window(u64),
    /// A monitor's usable work area: its bottom is the floor seat line.
    WorkArea(u64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopSurface {
    pub id: SurfaceId,
    pub rect: ScreenRect,
    /// Smaller values are closer to the user. Ties retain snapshot order.
    pub z_order: i32,
}
impl DesktopSurface {
    fn seat_y(self) -> f32 {
        match self.id {
            SurfaceId::Window(_) => self.rect.origin.y,
            SurfaceId::WorkArea(_) => self.rect.origin.y + self.rect.size.y,
        }
    }
    fn excluded(self, own_ids: &[u64]) -> bool {
        matches!(self.id, SurfaceId::Window(id) if own_ids.contains(&id))
    }
}

/// Every value uses the same logical screen coordinate space. `hips_offset` is
/// the projected live skeleton hips relative to the client window origin, not a
/// fixed fraction of the avatar rectangle. The caller validates the initial hit.
pub struct InteractionFrame<'a> {
    pub avatar_origin: Vec2,
    pub avatar_size: Vec2,
    pub hips_offset: Vec2,
    /// Projected avatar root in the same client coordinates. Sitting keeps the
    /// hips 10% of their root-relative height above the supporting edge.
    pub root_offset: Vec2,
    /// Lowest projected foot/toe bone relative to the client origin. This is an
    /// actual body contact point, independent of transparent viewport margins.
    pub feet_offset: Option<Vec2>,
    /// Projected logical pixels per avatar world unit (for the 0.3-unit snap).
    pub pixels_per_unit: f32,
    pub surfaces: &'a [DesktopSurface],
    pub own_window_ids: &'a [u64],
}
impl InteractionFrame<'_> {
    fn valid(&self) -> bool {
        valid_point(self.avatar_origin)
            && valid_point(self.avatar_size)
            && self.avatar_size.min_element() > 0.
            && valid_point(self.hips_offset)
            && valid_point(self.root_offset)
            && self.feet_offset.is_none_or(valid_point)
    }
    fn seat_offset(&self) -> Vec2 {
        Vec2::new(
            self.hips_offset.x,
            self.hips_offset.y
                + (self.root_offset.y - self.hips_offset.y).max(0.) * (1. - SITTING_ADJUST),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InteractionState {
    #[default]
    Idle,
    Dragging,
    Sitting,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InteractionInput {
    Start {
        cursor: Vec2,
    },
    Move {
        cursor: Vec2,
    },
    End {
        cursor: Vec2,
    },
    /// Native/compositor drag ended. The fresh frame origin is authoritative;
    /// cursor minus the original grab offset may differ after native constraints.
    NativeEnd {
        cursor: Vec2,
    },
    /// Lost capture, leaving desktop mode, or explicit user cancellation.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionOutput {
    pub window_origin: Vec2,
    /// Host uses this for idle / drag / sitting animation or procedural pose.
    pub state: InteractionState,
    pub surface: Option<SurfaceId>,
    pub hips_anchor: Option<Vec2>,
}

#[derive(Clone, Copy, Debug)]
struct Seat {
    surface: SurfaceId,
    fraction: f32,
    smoothed_hips: Vec2,
}

#[derive(Resource, Debug, Default)]
pub struct DesktopInteraction {
    pub state: InteractionState,
    grab_offset: Vec2,
    drag_origin: Vec2,
    seat: Option<Seat>,
}

fn valid_point(point: Vec2) -> bool {
    point.is_finite() && point.abs().max_element() <= MAX_COORDINATE
}

impl DesktopInteraction {
    /// Handle primary-pointer events after the host's avatar hit test. Move/End
    /// without a Start are ignored; Cancel safely clears both drag and seat.
    pub fn handle(
        &mut self,
        input: InteractionInput,
        frame: &InteractionFrame<'_>,
    ) -> InteractionOutput {
        if matches!(input, InteractionInput::Cancel) || !frame.valid() {
            self.reset();
            return self.output(frame.avatar_origin, None);
        }
        match input {
            InteractionInput::Start { cursor } if valid_point(cursor) => {
                self.grab_offset = cursor - frame.avatar_origin;
                self.drag_origin = frame.avatar_origin;
                self.seat = None;
                self.state = InteractionState::Dragging;
            }
            InteractionInput::Move { cursor }
            | InteractionInput::End { cursor }
            | InteractionInput::NativeEnd { cursor }
                if self.state == InteractionState::Dragging && valid_point(cursor) =>
            {
                let origin = if matches!(input, InteractionInput::NativeEnd { .. }) {
                    frame.avatar_origin
                } else {
                    cursor - self.grab_offset
                };
                if valid_point(origin) {
                    self.drag_origin = origin;
                }
                if matches!(
                    input,
                    InteractionInput::End { .. } | InteractionInput::NativeEnd { .. }
                ) {
                    let hips = self.drag_origin + frame.hips_offset;
                    self.seat =
                        choose_surface(frame, self.drag_origin, cursor).map(|surface| Seat {
                            surface: surface.id,
                            fraction: ((hips.x - surface.rect.origin.x) / surface.rect.size.x)
                                .clamp(0., 1.),
                            smoothed_hips: frame.seat_offset(),
                        });
                    self.state = if self.seat.is_some() {
                        InteractionState::Sitting
                    } else {
                        InteractionState::Idle
                    };
                    // Keep the release position even when the host has not yet
                    // applied the last pointer movement to its native window.
                    if self.seat.is_none() {
                        return self.output(self.drag_origin, None);
                    }
                }
            }
            _ => {}
        }
        self.tick(frame, 0.)
    }

    /// Call every frame with a fresh host snapshot. Follows movement/resizing,
    /// blends animation hips, and relinquishes a missing/invalid surface.
    pub fn tick(&mut self, frame: &InteractionFrame<'_>, dt_secs: f32) -> InteractionOutput {
        if !frame.valid() {
            self.reset();
            return self.output(frame.avatar_origin, None);
        }
        if self.state == InteractionState::Dragging {
            return self.output(self.drag_origin, None);
        }
        let Some(mut seat) = self.seat else {
            return self.output(frame.avatar_origin, None);
        };
        let Some(surface) = frame.surfaces.iter().find(|surface| {
            surface.id == seat.surface
                && surface.rect.valid()
                && !surface.excluded(frame.own_window_ids)
        }) else {
            self.reset();
            return self.output(frame.avatar_origin, None);
        };
        let scale = if frame.pixels_per_unit.is_finite() && frame.pixels_per_unit > 0. {
            frame.pixels_per_unit.clamp(0.01, MAX_COORDINATE)
        } else {
            100.
        };
        let seat_offset = frame.seat_offset();
        if (seat.smoothed_hips - seat_offset).abs().max_element() > HIPS_SNAP_WORLD_UNITS * scale {
            seat.smoothed_hips = seat_offset;
        } else {
            let dt = if dt_secs.is_finite() {
                dt_secs.clamp(0., 1.)
            } else {
                0.
            };
            seat.smoothed_hips = seat
                .smoothed_hips
                .lerp(seat_offset, 1. - (-HIPS_BLEND_SPEED * dt).exp());
        }
        let anchor = Vec2::new(
            surface.rect.origin.x + surface.rect.size.x * seat.fraction,
            surface.seat_y(),
        );
        let origin = anchor - seat.smoothed_hips;
        if !valid_point(origin) {
            self.reset();
            return self.output(frame.avatar_origin, None);
        }
        self.seat = Some(seat);
        self.output(origin, Some(anchor))
    }

    /// Resolve an explicit sit request against an exposed trusted surface touching
    /// the projected hips. No remembered/persisted pose can create a seat.
    pub fn sit_on_surface_below(&mut self, frame: &InteractionFrame<'_>) -> InteractionOutput {
        self.reset();
        if !frame.valid() {
            return self.output(frame.avatar_origin, None);
        }
        let hips = frame.avatar_origin + frame.hips_offset;
        // A request can confirm contact, never search down the screen and
        // teleport to an unrelated window or the taskbar.
        let selected = choose_surface(frame, frame.avatar_origin, hips);
        if let Some(surface) = selected {
            self.state = InteractionState::Sitting;
            self.seat = Some(Seat {
                surface: surface.id,
                fraction: ((hips.x - surface.rect.origin.x) / surface.rect.size.x).clamp(0., 1.),
                smoothed_hips: frame.seat_offset(),
            });
        }
        self.tick(frame, 0.)
    }

    /// Leave the seat standing on its edge. The projected root is the avatar's
    /// pose-independent ground point; keeping it on the seat line stops the
    /// standing legs from dropping through the window top or panel. Without a
    /// live seat this is a plain cancel at the current origin.
    pub fn stand_up(&mut self, frame: &InteractionFrame<'_>) -> InteractionOutput {
        let seat_y = self.seat.and_then(|seat| {
            frame
                .surfaces
                .iter()
                .find(|surface| {
                    surface.id == seat.surface
                        && surface.rect.valid()
                        && !surface.excluded(frame.own_window_ids)
                })
                .map(|surface| surface.seat_y())
        });
        self.reset();
        let origin = seat_y
            .filter(|_| frame.valid())
            .map(|seat_y| Vec2::new(frame.avatar_origin.x, seat_y - frame.root_offset.y))
            .filter(|origin| valid_point(*origin))
            .unwrap_or(frame.avatar_origin);
        self.output(origin, None)
    }

    fn reset(&mut self) {
        self.state = InteractionState::Idle;
        self.seat = None;
    }
    fn output(&self, origin: Vec2, anchor: Option<Vec2>) -> InteractionOutput {
        InteractionOutput {
            window_origin: origin,
            state: self.state,
            surface: self.seat.map(|seat| seat.surface),
            hips_anchor: anchor,
        }
    }
}

/// Preserve Homunculus' 80px cursor band above exposed window edges. Monitor and
/// taskbar floors additionally accept actual hips/feet contact: the user can grab
/// the torso while dropping the body by the taskbar, or release on the panel.
/// Floors are considered only when no exposed window edge qualifies.
fn choose_surface<'a>(
    frame: &'a InteractionFrame<'_>,
    avatar_origin: Vec2,
    cursor: Vec2,
) -> Option<&'a DesktopSurface> {
    let eligible = |surface: &&DesktopSurface| {
        if !surface.rect.valid() || surface.excluded(frame.own_window_ids) {
            return false;
        }
        let rect = surface.rect;
        let overlap = avatar_origin.x < rect.origin.x + rect.size.x
            && avatar_origin.x + frame.avatar_size.x > rect.origin.x;
        let distance = surface.seat_y() - cursor.y;
        overlap
            && cursor.x >= rect.origin.x
            && cursor.x <= rect.origin.x + rect.size.x
            && (0. ..=EDGE_THRESHOLD).contains(&distance)
    };
    let exposed_window = |surface: &&DesktopSurface| {
        if !matches!(surface.id, SurfaceId::Window(_)) || !eligible(surface) {
            return false;
        }
        let edge = Vec2::new(cursor.x, surface.seat_y());
        !frame.surfaces.iter().any(|front| {
            matches!(front.id, SurfaceId::Window(_))
                && front.id != surface.id
                && front.z_order < surface.z_order
                && !front.excluded(frame.own_window_ids)
                && front.rect.valid()
                && (front.rect.contains(cursor) || front.rect.contains(edge))
        })
    };
    let floor_distance = |surface: &DesktopSurface| -> Option<f32> {
        if !surface.rect.valid() || !matches!(surface.id, SurfaceId::WorkArea(_)) {
            return None;
        }
        let rect = surface.rect;
        if avatar_origin.x >= rect.origin.x + rect.size.x
            || avatar_origin.x + frame.avatar_size.x <= rect.origin.x
        {
            return None;
        }
        let hips = avatar_origin + frame.hips_offset;
        [
            Some(cursor),
            Some(hips),
            frame.feet_offset.map(|feet| avatar_origin + feet),
        ]
        .into_iter()
        .flatten()
        .filter(|point| point.x >= rect.origin.x && point.x <= rect.origin.x + rect.size.x)
        .map(|point| (surface.seat_y() - point.y).abs())
        .filter(|distance| *distance <= EDGE_THRESHOLD)
        .min_by(f32::total_cmp)
    };
    frame
        .surfaces
        .iter()
        .filter(exposed_window)
        .min_by_key(|surface| surface.z_order)
        .or_else(|| {
            frame
                .surfaces
                .iter()
                .filter_map(|surface| floor_distance(surface).map(|distance| (surface, distance)))
                .min_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(surface, _)| surface)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: SurfaceId, origin: Vec2, size: Vec2, z_order: i32) -> DesktopSurface {
        DesktopSurface {
            id,
            rect: ScreenRect { origin, size },
            z_order,
        }
    }
    fn frame(surfaces: &[DesktopSurface]) -> InteractionFrame<'_> {
        InteractionFrame {
            avatar_origin: Vec2::new(150., 100.),
            avatar_size: Vec2::new(200., 300.),
            hips_offset: Vec2::new(100., 200.),
            root_offset: Vec2::new(100., 200.),
            feet_offset: None,
            pixels_per_unit: 100.,
            surfaces,
            own_window_ids: &[],
        }
    }
    fn drop_on(
        controller: &mut DesktopInteraction,
        frame: &InteractionFrame<'_>,
        cursor: Vec2,
    ) -> InteractionOutput {
        controller.handle(InteractionInput::Start { cursor }, frame);
        controller.handle(InteractionInput::End { cursor }, frame)
    }

    #[test]
    fn dragging_preserves_grab_offset_and_end_without_start_is_ignored() {
        let frame = frame(&[]);
        let mut state = DesktopInteraction::default();
        assert_eq!(
            state
                .handle(InteractionInput::End { cursor: Vec2::ZERO }, &frame)
                .state,
            InteractionState::Idle
        );
        state.handle(
            InteractionInput::Start {
                cursor: Vec2::new(180., 140.),
            },
            &frame,
        );
        let moved = state.handle(
            InteractionInput::Move {
                cursor: Vec2::new(-220., 340.),
            },
            &frame,
        );
        assert_eq!(moved.window_origin, Vec2::new(-250., 300.));
        let released = state.handle(
            InteractionInput::End {
                cursor: Vec2::new(-240., 350.),
            },
            &frame,
        );
        assert_eq!(released.window_origin, Vec2::new(-270., 310.));
        assert_eq!(released.state, InteractionState::Idle);
    }

    #[test]
    fn seat_tracks_window_move_resize_with_live_hips_and_missing_surface_restores_idle() {
        let mut surfaces = [surface(
            SurfaceId::Window(5),
            Vec2::new(100., 400.),
            Vec2::new(600., 400.),
            0,
        )];
        let mut state = DesktopInteraction::default();
        let output = drop_on(&mut state, &frame(&surfaces), Vec2::new(250., 350.));
        assert_eq!(output.state, InteractionState::Sitting);
        assert_eq!(output.window_origin, Vec2::new(150., 200.));
        surfaces[0].rect.origin = Vec2::new(-900., 200.);
        surfaces[0].rect.size.x = 1200.;
        let output = state.tick(&frame(&surfaces), 0.016);
        assert_eq!(output.hips_anchor, Some(Vec2::new(-600., 200.)));
        assert_eq!(output.window_origin, Vec2::new(-700., 0.));
        assert_eq!(state.tick(&frame(&[]), 0.016).state, InteractionState::Idle);
    }

    #[test]
    fn topmost_occlusion_own_windows_and_exact_threshold() {
        let back = surface(
            SurfaceId::Window(1),
            Vec2::new(100., 400.),
            Vec2::new(600., 400.),
            4,
        );
        let front = surface(
            SurfaceId::Window(2),
            Vec2::new(100., 390.),
            Vec2::new(600., 400.),
            0,
        );
        let own = surface(
            SurfaceId::Window(99),
            Vec2::new(100., 380.),
            Vec2::new(600., 400.),
            -1,
        );
        let surfaces = [back, own, front];
        let mut frame = frame(&surfaces);
        frame.own_window_ids = &[99];
        let mut state = DesktopInteraction::default();
        assert_eq!(
            drop_on(&mut state, &frame, Vec2::new(250., 320.)).surface,
            Some(SurfaceId::Window(2))
        );
        // Behind the front window's body, the back edge must not qualify.
        assert_eq!(
            drop_on(&mut state, &frame, Vec2::new(250., 395.)).state,
            InteractionState::Idle
        );
        let surfaces = [back];
        let frame = super::tests::frame(&surfaces);
        assert_eq!(
            drop_on(&mut state, &frame, Vec2::new(250., 320.)).state,
            InteractionState::Sitting
        );
        assert_eq!(
            drop_on(&mut state, &frame, Vec2::new(250., 319.9)).state,
            InteractionState::Idle
        );
    }

    #[test]
    fn workarea_floor_negative_monitor_and_start_detaches_seat() {
        let surfaces = [surface(
            SurfaceId::WorkArea(3),
            Vec2::new(-1920., -200.),
            Vec2::new(1920., 1000.),
            100,
        )];
        let frame = InteractionFrame {
            avatar_origin: Vec2::new(-500., 400.),
            ..frame(&surfaces)
        };
        let mut state = DesktopInteraction::default();
        let out = drop_on(&mut state, &frame, Vec2::new(-400., 750.));
        assert_eq!(out.hips_anchor, Some(Vec2::new(-400., 800.)));
        assert_eq!(out.window_origin, Vec2::new(-500., 600.));
        let out = state.handle(
            InteractionInput::Start {
                cursor: Vec2::new(-400., 750.),
            },
            &frame,
        );
        assert_eq!(out.state, InteractionState::Dragging);
        assert_eq!(out.surface, None);
        assert_eq!(
            state.handle(InteractionInput::Cancel, &frame).state,
            InteractionState::Idle
        );
    }

    #[test]
    fn taskbar_drop_uses_real_feet_when_the_grabbed_torso_cursor_is_far_above_it() {
        let surfaces = [
            surface(
                SurfaceId::WorkArea(1),
                Vec2::ZERO,
                Vec2::new(1920., 1080.),
                100,
            ),
            surface(
                SurfaceId::WorkArea(2),
                Vec2::new(250., 0.),
                Vec2::new(600., 1032.),
                100,
            ),
        ];
        let frame = InteractionFrame {
            avatar_origin: Vec2::new(200., 600.),
            feet_offset: Some(Vec2::new(100., 420.)),
            ..frame(&surfaces)
        };
        let out = drop_on(
            &mut DesktopInteraction::default(),
            &frame,
            Vec2::new(300., 720.),
        );
        assert_eq!(out.surface, Some(SurfaceId::WorkArea(2)));
        assert_eq!(out.hips_anchor, Some(Vec2::new(300., 1032.)));
        assert_eq!(out.window_origin, Vec2::new(200., 832.));
    }

    #[test]
    fn cursor_on_the_taskbar_qualifies_but_distant_body_and_wrong_dock_span_do_not() {
        let surfaces = [surface(
            SurfaceId::WorkArea(2),
            Vec2::new(250., 0.),
            Vec2::new(600., 1032.),
            100,
        )];
        let mut frame = InteractionFrame {
            avatar_origin: Vec2::new(200., 600.),
            ..frame(&surfaces)
        };
        assert_eq!(
            drop_on(
                &mut DesktopInteraction::default(),
                &frame,
                Vec2::new(300., 1050.)
            )
            .surface,
            Some(SurfaceId::WorkArea(2))
        );
        frame.feet_offset = Some(Vec2::new(100., 300.));
        assert_eq!(
            drop_on(
                &mut DesktopInteraction::default(),
                &frame,
                Vec2::new(300., 720.)
            )
            .state,
            InteractionState::Idle
        );
        // The viewport overlaps the dock, but cursor and actual body contacts
        // are outside its width. Transparent margins must not create a seat.
        frame.avatar_origin.x = 100.;
        frame.feet_offset = Some(Vec2::new(100., 420.));
        assert_eq!(
            drop_on(
                &mut DesktopInteraction::default(),
                &frame,
                Vec2::new(200., 720.)
            )
            .state,
            InteractionState::Idle
        );
    }

    #[test]
    fn feet_near_a_window_do_not_expand_homunculus_cursor_window_eligibility() {
        let surfaces = [surface(
            SurfaceId::Window(2),
            Vec2::new(250., 1032.),
            Vec2::new(600., 400.),
            0,
        )];
        let frame = InteractionFrame {
            avatar_origin: Vec2::new(200., 600.),
            feet_offset: Some(Vec2::new(100., 420.)),
            ..frame(&surfaces)
        };
        assert_eq!(
            drop_on(
                &mut DesktopInteraction::default(),
                &frame,
                Vec2::new(300., 720.)
            )
            .state,
            InteractionState::Idle
        );
    }

    #[test]
    fn native_end_preserves_the_compositors_actual_origin_and_horizontal_seat_anchor() {
        let mut state = DesktopInteraction::default();
        let initial = frame(&[]);
        state.handle(
            InteractionInput::Start {
                cursor: Vec2::new(180., 140.),
            },
            &initial,
        );
        let release = InteractionFrame {
            avatar_origin: Vec2::new(-200., 600.),
            ..frame(&[])
        };
        let out = state.handle(
            InteractionInput::NativeEnd {
                cursor: Vec2::new(-150., 900.),
            },
            &release,
        );
        assert_eq!(out.state, InteractionState::Idle);
        assert_eq!(out.window_origin, release.avatar_origin);

        let surfaces = [surface(
            SurfaceId::WorkArea(7),
            Vec2::new(-1920., 0.),
            Vec2::new(1920., 800.),
            100,
        )];
        let release = InteractionFrame {
            surfaces: &surfaces,
            ..release
        };
        state.handle(
            InteractionInput::Start {
                cursor: Vec2::new(180., 140.),
            },
            &initial,
        );
        let out = state.handle(
            InteractionInput::NativeEnd {
                cursor: Vec2::new(-150., 900.),
            },
            &release,
        );
        assert_eq!(out.hips_anchor, Some(Vec2::new(-100., 800.)));
        assert_eq!(out.window_origin, Vec2::new(-200., 600.));
    }

    #[test]
    fn hips_blend_is_time_based_and_large_model_change_snaps() {
        let surfaces = [surface(
            SurfaceId::Window(1),
            Vec2::new(100., 400.),
            Vec2::new(600., 400.),
            0,
        )];
        let mut frame = frame(&surfaces);
        let mut state = DesktopInteraction::default();
        drop_on(&mut state, &frame, Vec2::new(250., 350.));
        frame.hips_offset.y += 20.;
        let out = state.tick(&frame, 0.25);
        let expected = 400. - (200. + 20. * (1. - (-1_f32).exp()));
        assert!((out.window_origin.y - expected).abs() < 0.001);
        frame.hips_offset.y = 400.;
        assert_eq!(state.tick(&frame, 0.).window_origin.y, 0.);
        frame.hips_offset.y = f32::NAN;
        assert_eq!(
            state.tick(&frame, f32::INFINITY).state,
            InteractionState::Idle
        );
    }

    #[test]
    fn seated_hips_keep_homunculus_ten_percent_clearance_and_corrected_anchor_smoothing() {
        let surfaces = [surface(
            SurfaceId::Window(1),
            Vec2::new(100., 400.),
            Vec2::new(600., 400.),
            0,
        )];
        let mut frame = frame(&surfaces);
        // The actual seated VRMA's 0.824m hips at 200 logical px per metre.
        frame.root_offset.y = frame.hips_offset.y + 0.824 * 200.;
        frame.pixels_per_unit = 200.;
        let mut state = DesktopInteraction::default();
        let seated = drop_on(&mut state, &frame, Vec2::new(250., 350.));
        let live_hips_y = seated.window_origin.y + frame.hips_offset.y;
        assert!((400. - live_hips_y - 16.48).abs() < 0.001);
        frame.hips_offset.y += 20.;
        let moved = state.tick(&frame, 0.25);
        // Changing live hips by 20px changes the adjusted contact by 18px.
        let expected_contact = 216.48 + 18. * (1. - (-1_f32).exp());
        assert!((moved.window_origin.y - (400. - expected_contact)).abs() < 0.001);
        state.handle(
            InteractionInput::Start {
                cursor: Vec2::new(250., 350.),
            },
            &frame,
        );
        let dragged = state.handle(
            InteractionInput::Move {
                cursor: Vec2::new(280., 375.),
            },
            &frame,
        );
        assert_eq!(
            dragged.window_origin,
            frame.avatar_origin + Vec2::new(30., 25.)
        );
    }

    #[test]
    fn stand_up_keeps_the_projected_root_on_the_seat_edge_and_never_invents_one() {
        let surfaces = [
            surface(
                SurfaceId::Window(1),
                Vec2::new(100., 400.),
                Vec2::new(600., 400.),
                0,
            ),
            surface(
                SurfaceId::WorkArea(3),
                Vec2::new(-1920., 0.),
                Vec2::new(3840., 1032.),
                i32::MAX,
            ),
        ];
        let mut frame = frame(&surfaces);
        frame.root_offset.y = 380.;
        let mut state = DesktopInteraction::default();
        let seated = drop_on(&mut state, &frame, Vec2::new(250., 350.));
        assert_eq!(seated.surface, Some(SurfaceId::Window(1)));
        frame.avatar_origin = seated.window_origin;
        let stood = state.stand_up(&frame);
        assert_eq!(stood.state, InteractionState::Idle);
        assert!(stood.surface.is_none() && stood.hips_anchor.is_none());
        assert_eq!(stood.window_origin, Vec2::new(seated.window_origin.x, 20.));
        assert_eq!(stood.window_origin.y + frame.root_offset.y, 400.);
        // Already standing: a plain no-op at the current origin.
        frame.avatar_origin = stood.window_origin;
        assert_eq!(state.stand_up(&frame).window_origin, stood.window_origin);

        // Floor seat: feet land on the work-area bottom.
        frame.avatar_origin = Vec2::new(-500., 400.);
        let seated = drop_on(&mut state, &frame, Vec2::new(-400., 1000.));
        assert_eq!(seated.surface, Some(SurfaceId::WorkArea(3)));
        frame.avatar_origin = seated.window_origin;
        assert_eq!(state.stand_up(&frame).window_origin.y, 1032. - 380.);

        // A vanished, own, or invalid seat never moves the window.
        let seat = &surfaces[..1];
        for (live, own, root_y) in [
            (&[][..], &[][..], 380.),
            (seat, &[1_u64][..], 380.),
            (seat, &[][..], f32::NAN),
        ] {
            let mut frame = super::tests::frame(seat);
            frame.root_offset.y = 380.;
            let mut state = DesktopInteraction::default();
            drop_on(&mut state, &frame, Vec2::new(250., 350.));
            assert_eq!(state.state, InteractionState::Sitting);
            frame.surfaces = live;
            frame.own_window_ids = own;
            frame.root_offset.y = root_y;
            let out = state.stand_up(&frame);
            assert_eq!(out.state, InteractionState::Idle);
            assert_eq!(out.window_origin, frame.avatar_origin);
        }
    }

    #[test]
    fn explicit_sit_requires_edge_contact_and_never_searches_for_a_distant_seat() {
        let surfaces = [
            surface(
                SurfaceId::Window(1),
                Vec2::new(100., 400.),
                Vec2::new(600., 400.),
                2,
            ),
            surface(
                SurfaceId::Window(2),
                Vec2::new(100., 390.),
                Vec2::new(600., 400.),
                0,
            ),
            surface(
                SurfaceId::Window(99),
                Vec2::new(100., 350.),
                Vec2::new(600., 400.),
                -1,
            ),
            surface(
                SurfaceId::WorkArea(3),
                Vec2::ZERO,
                Vec2::new(1920., 1080.),
                i32::MAX,
            ),
        ];
        let mut frame = frame(&surfaces);
        frame.own_window_ids = &[99];
        let mut state = DesktopInteraction::default();
        assert_eq!(
            state.sit_on_surface_below(&frame).state,
            InteractionState::Idle
        );
        frame.avatar_origin.y = 390. - frame.hips_offset.y - 10.;
        let seated = state.sit_on_surface_below(&frame);
        assert_eq!(seated.surface, Some(SurfaceId::Window(2)));
        // No surfaces below the current projected hips: no pose/anchor survives.
        frame.avatar_origin.y = 1200.;
        let failed = state.sit_on_surface_below(&frame);
        assert_eq!(failed.state, InteractionState::Idle);
        assert!(failed.surface.is_none() && failed.hips_anchor.is_none());
        frame.hips_offset.y = f32::NAN;
        assert_eq!(
            state.sit_on_surface_below(&frame).state,
            InteractionState::Idle
        );
    }
}
