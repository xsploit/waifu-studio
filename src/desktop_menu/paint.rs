//! Original vector artwork for the desktop wheel. No external icon/font assets.
use super::{Action, INNER, RADIUS, label};
use bevy_egui::egui::{self, Color32, Pos2, Stroke, Vec2};
use std::f32::consts::{FRAC_PI_2, TAU};

const ACCENT: Color32 = Color32::from_rgb(188, 168, 249);
const TEXT: Color32 = Color32::from_rgb(235, 233, 243);
const MUTED: Color32 = Color32::from_rgb(149, 149, 167);

pub(super) struct Wheel<'a> {
    pub center: Pos2,
    pub radius: f32,
    pub items: &'a [Action],
    pub active: Option<usize>,
    pub sitting: bool,
    pub recording: bool,
    pub voice_enabled: bool,
    pub voice_page: bool,
    pub name: &'a str,
    pub opened_at: f64,
}

fn title(action: Action, wheel: &Wheel<'_>) -> &'static str {
    match action {
        Action::Microphone if wheel.recording => "Stop mic",
        Action::Microphone => "Start mic",
        Action::ToggleVoice if wheel.voice_enabled => "Mute replies",
        Action::ToggleVoice => "Unmute",
        Action::Character => "Profiles",
        Action::Passthrough => "Pass clicks",
        Action::Reframe => "Recenter",
        _ => label(action, wheel.sitting),
    }
}
fn hint(action: Action, wheel: &Wheel<'_>) -> &'static str {
    match action {
        Action::Settings => "Open settings",
        Action::Character => "Switch profile",
        Action::Personality => "Switch personality",
        Action::Voice => "Voice controls",
        Action::Sitting if wheel.sitting => "Back on your feet",
        Action::Sitting => "Take a seat",
        Action::Passthrough => "Ignore mouse clicks",
        Action::Studio => "Chat & create",
        Action::Reframe => "Reset framing",
        Action::Close => "Dismiss menu",
        Action::Microphone if wheel.recording => "Finish recording",
        Action::Microphone => "Record a message",
        Action::ToggleVoice if wheel.voice_enabled => "Silence replies",
        Action::ToggleVoice => "Speak replies",
        Action::StopVoice => "Stop & cancel",
        Action::VoiceSettings => "Voice & shortcuts",
    }
}

pub(super) fn wheel(painter: &egui::Painter, wheel: &Wheel<'_>) {
    let ctx = painter.ctx();
    let duration = ctx.style_of(egui::Theme::Dark).animation_time.min(0.16);
    let age = (ctx.input(|i| i.time) - wheel.opened_at).max(0.0) as f32;
    let progress = if duration > 0.0 {
        (age / duration).min(1.0)
    } else {
        1.0
    };
    let ease = 1.0 - (1.0 - progress).powi(3);
    if progress < 1.0 {
        ctx.request_repaint();
    }
    let mut painter = painter.clone();
    painter.multiply_opacity(0.4 + ease * 0.6);
    let radius = wheel.radius * (0.94 + ease * 0.06);
    let scale = radius / RADIUS;
    let center = wheel.center;
    let inner = INNER * scale;
    // Feather the rim without a large opaque rectangle over the character.
    for (extra, alpha) in [(4.0, 10), (2.5, 20), (1.0, 40)] {
        painter.circle_filled(center, radius + extra, Color32::from_black_alpha(alpha));
    }
    painter.circle_filled(
        center,
        radius,
        Color32::from_rgba_unmultiplied(21, 22, 29, 247),
    );
    let count = wheel.items.len() as f32;
    for (index, action) in wheel.items.iter().copied().enumerate() {
        let middle = index as f32 * TAU / count - FRAC_PI_2;
        let start = middle - TAU / (count * 2.0) + 0.013;
        let end = middle + TAU / (count * 2.0) - 0.013;
        let hover = ctx.animate_bool_with_time(
            egui::Id::new(("desktop_wedge", wheel.voice_page, index)),
            wheel.active == Some(index),
            duration.min(0.10),
        );
        let fill = Color32::from_rgb(
            (31.0 + hover * 39.0) as u8,
            (32.0 + hover * 26.0) as u8,
            (42.0 + hover * 53.0) as u8,
        );
        sector(
            &painter,
            center,
            inner + 3.0 * scale,
            radius - 3.0,
            start,
            end,
            fill,
        );
        if hover > 0.01 {
            sector(
                &painter,
                center,
                radius - 4.0,
                radius - 2.0,
                start + 0.025,
                end - 0.025,
                ACCENT.gamma_multiply(hover),
            );
        }
        let color = if wheel.active == Some(index) {
            Color32::WHITE
        } else {
            TEXT
        };
        let at = center + Vec2::angled(middle) * radius * 0.72;
        let compact = wheel.radius < 112.0;
        let icon_at = at - Vec2::new(0.0, if compact { 0.0 } else { 7.0 * scale });
        icon(&painter, action, icon_at, 10.5 * scale, color);
        if !compact {
            painter.text(
                at + Vec2::new(0.0, 14.0 * scale),
                egui::Align2::CENTER_CENTER,
                title(action, wheel),
                egui::FontId::proportional(10.5 * scale),
                color,
            );
        }
    }
    painter.circle_stroke(
        center,
        radius - 0.5,
        Stroke::new(0.75, Color32::from_white_alpha(28)),
    );
    painter.circle_filled(center, inner, Color32::from_rgb(26, 27, 35));
    painter.circle_stroke(
        center,
        inner,
        Stroke::new(1.0, Color32::from_white_alpha(25)),
    );
    let active = wheel.active.map(|index| wheel.items[index]);
    if let Some(action) = active {
        icon(
            &painter,
            action,
            center - Vec2::new(0.0, 18.0 * scale),
            8.5 * scale,
            ACCENT,
        );
    } else {
        painter.circle_filled(center - Vec2::new(0.0, 19.0 * scale), 2.5 * scale, ACCENT);
    }
    let caption = active.map_or(
        if wheel.voice_page {
            "Voice"
        } else {
            "Companion"
        },
        |a| title(a, wheel),
    );
    centered(
        &painter,
        center,
        caption,
        (inner * 1.8).max(20.0),
        (11.5 * scale).max(9.0),
        TEXT,
    );
    let detail = active.map_or(wheel.name, |a| hint(a, wheel));
    if wheel.radius >= 112.0 {
        centered(
            &painter,
            center + Vec2::new(0.0, 17.0 * scale),
            detail,
            inner * 1.8,
            8.5 * scale,
            MUTED,
        );
    }
}

fn centered(p: &egui::Painter, at: Pos2, text: &str, width: f32, size: f32, color: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.into(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    let galley = p.layout_job(job);
    p.galley(at - galley.size() * 0.5, galley, color);
}
#[allow(clippy::too_many_arguments)]
fn sector(
    p: &egui::Painter,
    center: Pos2,
    inner: f32,
    outer: f32,
    start: f32,
    end: f32,
    color: Color32,
) {
    let mut mesh = egui::Mesh::default();
    for step in 0..=24_u32 {
        let direction = Vec2::angled(start + (end - start) * step as f32 / 24.0);
        mesh.colored_vertex(center + direction * inner, color);
        mesh.colored_vertex(center + direction * outer, color);
        if step > 0 {
            let end = step * 2;
            mesh.add_triangle(end - 2, end - 1, end);
            mesh.add_triangle(end - 1, end + 1, end);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}
fn icon(p: &egui::Painter, action: Action, at: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new((size * 0.15).max(1.0), color);
    let point = |x: f32, y: f32| at + Vec2::new(x, y) * size;
    let line = |coords: &[(f32, f32)]| {
        p.line(coords.iter().map(|&(x, y)| point(x, y)).collect(), stroke);
    };
    let circle = |x, y, radius| {
        p.circle_stroke(point(x, y), radius * size, stroke);
    };
    match action {
        Action::Settings | Action::VoiceSettings => {
            if matches!(action, Action::Settings) {
                circle(0.0, 0.0, 0.64);
                circle(0.0, 0.0, 0.23);
                for i in 0..8 {
                    let d = Vec2::angled(i as f32 * TAU / 8.0);
                    p.line_segment([at + d * size * 0.65, at + d * size * 0.94], stroke);
                }
            } else {
                for (x, y) in [(-0.65, -0.35), (0.0, 0.4), (0.65, -0.1)] {
                    line(&[(x, -0.85), (x, y - 0.2)]);
                    line(&[(x, y + 0.2), (x, 0.85)]);
                    circle(x, y, 0.18);
                }
            }
        }
        Action::Personality => {
            circle(0.0, 0.0, 0.85);
            p.circle_filled(point(-0.3, -0.2), size * 0.08, color);
            p.circle_filled(point(0.3, -0.2), size * 0.08, color);
            line(&[(-0.35, 0.25), (-0.15, 0.43), (0.15, 0.43), (0.35, 0.25)]);
        }
        Action::Character => {
            circle(0.0, -0.4, 0.35);
            line(&[
                (-0.75, 0.85),
                (-0.65, 0.25),
                (-0.3, 0.08),
                (0.3, 0.08),
                (0.65, 0.25),
                (0.75, 0.85),
            ]);
        }
        Action::Microphone => {
            p.rect_stroke(
                egui::Rect::from_min_max(point(-0.28, -0.9), point(0.28, 0.3)),
                size * 0.28,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[
                (-0.6, -0.1),
                (-0.6, 0.25),
                (-0.35, 0.55),
                (0.35, 0.55),
                (0.6, 0.25),
                (0.6, -0.1),
            ]);
            line(&[(0.0, 0.55), (0.0, 0.95)]);
            line(&[(-0.35, 0.95), (0.35, 0.95)]);
        }
        Action::Voice | Action::ToggleVoice => {
            line(&[
                (-0.85, -0.3),
                (-0.5, -0.3),
                (0.0, -0.8),
                (0.0, 0.8),
                (-0.5, 0.3),
                (-0.85, 0.3),
                (-0.85, -0.3),
            ]);
            line(&[(0.3, -0.35), (0.48, 0.0), (0.3, 0.35)]);
            line(&[(0.6, -0.65), (0.88, 0.0), (0.6, 0.65)]);
        }
        Action::Sitting => {
            circle(-0.25, -0.75, 0.2);
            line(&[(-0.3, -0.3), (-0.3, 0.25), (0.45, 0.25), (0.65, 0.9)]);
            line(&[(-0.8, -0.45), (-0.8, 0.5), (0.35, 0.5)]);
            line(&[(-0.75, 0.5), (-0.75, 0.95)]);
        }
        Action::Passthrough => {
            line(&[
                (-0.65, -0.9),
                (-0.65, 0.65),
                (-0.25, 0.3),
                (0.0, 0.85),
                (0.3, 0.7),
                (0.05, 0.15),
                (0.6, 0.15),
                (-0.65, -0.9),
            ]);
            line(&[(0.35, -0.75), (0.85, -0.75), (0.85, -0.3)]);
        }
        Action::Studio => {
            p.rect_stroke(
                egui::Rect::from_min_max(point(-0.9, -0.7), point(0.9, 0.7)),
                size * 0.15,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(-0.9, -0.25), (0.9, -0.25)]);
            line(&[(-0.3, -0.25), (-0.3, 0.7)]);
        }
        Action::Reframe => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                line(&[
                    (x * 0.4, y * 0.85),
                    (x * 0.85, y * 0.85),
                    (x * 0.85, y * 0.4),
                ]);
            }
            circle(0.0, 0.0, 0.2);
        }
        Action::Close => {
            line(&[(-0.55, -0.55), (0.55, 0.55)]);
            line(&[(0.55, -0.55), (-0.55, 0.55)]);
        }
        Action::StopVoice => {
            p.rect_filled(
                egui::Rect::from_center_size(at, Vec2::splat(size * 1.25)),
                size * 0.15,
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_menu::{ACTIONS, VOICE_ACTIONS, radial_radius};

    #[test]
    fn wheel_paint_fits_and_produces_review_images_when_requested() {
        for (name, width, voice_page, active, recording, sitting) in [
            ("companion", 420.0, false, None, false, false),
            ("hover-voice", 420.0, false, Some(2), false, false),
            ("voice-recording", 420.0, true, Some(0), true, false),
            ("seated", 420.0, false, Some(3), false, true),
            ("compact", 210.0, false, Some(1), false, false),
            ("smallest", 170.0, false, Some(4), false, false),
            ("compact-voice", 210.0, true, Some(3), false, false),
        ] {
            let ctx = egui::Context::default();
            crate::ui::ensure_style(&ctx);
            let screen = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 360.0));
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(1.0),
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx();
                    let p = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("wheel-review"),
                    ));
                    wheel(
                        &p,
                        &Wheel {
                            center: screen.center(),
                            radius: radial_radius(screen),
                            items: if voice_page { &VOICE_ACTIONS } else { &ACTIONS },
                            active,
                            sitting,
                            recording,
                            voice_enabled: true,
                            voice_page,
                            name: "Hikari",
                            opened_at: 0.0,
                        },
                    );
                },
            );
            let primitives =
                ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
            for clipped in &primitives {
                if let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive {
                    assert!(mesh.is_valid());
                    for v in &mesh.vertices {
                        assert!(v.pos.x.is_finite() && v.pos.y.is_finite());
                        assert!(screen.expand(1.0).contains(v.pos), "{name}: {:?}", v.pos);
                    }
                }
            }
            if let Some(folder) = std::env::var_os("WAIFU_RADIAL_REVIEW_DIR") {
                let folder = std::path::PathBuf::from(folder);
                std::fs::create_dir_all(&folder).unwrap();
                crate::profile_library::write_review(
                    &output,
                    &primitives,
                    [width, 360.0],
                    &folder.join(format!("{name}.json")),
                );
            }
            output.textures_delta.clear();
        }
    }
}
