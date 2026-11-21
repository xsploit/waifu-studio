//! A second render view of the live avatar, docked beside visual settings.
use crate::studio::{Studio, Tab, ViewMode};
use bevy::{
    camera::{RenderTarget, primitives::Aabb},
    prelude::*,
    render::render_resource::TextureFormat,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};
use waifu_studio::loading::AvatarLoadState;

pub struct AvatarPreviewPlugin;
impl Plugin for AvatarPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AvatarPreview>()
            .add_systems(Startup, setup)
            .add_systems(Update, update);
    }
}
#[derive(Resource)]
pub struct AvatarPreview {
    texture: Option<egui::TextureId>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    size: UVec2,
    root: Option<Entity>,
    bounds: Option<(Vec3, Vec3)>,
    framing: Framing,
}
impl Default for AvatarPreview {
    fn default() -> Self {
        Self {
            texture: None,
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            size: UVec2::new(600, 900),
            root: None,
            bounds: None,
            framing: Framing::FullBody,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Framing {
    FullBody,
    UpperBody,
    Face,
}
impl Framing {
    fn bounds(self, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
        let height = max.y - min.y;
        let center = (max + min) * 0.5;
        match self {
            Self::FullBody => (min, max),
            Self::UpperBody => (
                Vec3::new(center.x - height * 0.29, min.y + height * 0.42, min.z),
                Vec3::new(center.x + height * 0.29, max.y, max.z),
            ),
            Self::Face => (
                Vec3::new(center.x - height * 0.17, min.y + height * 0.72, min.z),
                Vec3::new(center.x + height * 0.17, max.y, max.z),
            ),
        }
    }
}
#[derive(Component)]
struct PreviewCamera;
#[derive(Resource)]
struct PreviewImage(Handle<Image>);
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
    mut preview: ResMut<AvatarPreview>,
) {
    let image = images.add(Image::new_target_texture(
        600,
        900,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    preview.texture = Some(textures.add_image(EguiTextureHandle::Strong(image.clone())));
    commands.spawn((
        PreviewCamera,
        waifu_studio::visuals::VisualCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.065, 0.062)),
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        Projection::Perspective(PerspectiveProjection {
            fov: 35_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.9, 3.8).looking_at(Vec3::Y * 0.9, Vec3::Y),
    ));
    commands.insert_resource(PreviewImage(image));
}

fn frame(min: Vec3, max: Vec3, aspect: f32, zoom: f32, yaw: f32, pitch: f32) -> Transform {
    let extent = (max - min).max(Vec3::splat(0.01));
    let focus = (min + max) * 0.5;
    let half_fov = 17.5_f32.to_radians();
    let radius = ((extent.y * 0.58 / half_fov.tan())
        .max(extent.x * 0.58 / (half_fov.tan() * aspect.max(0.1)))
        + extent.z * 0.5)
        / zoom;
    let direction = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0) * Vec3::Z;
    Transform::from_translation(focus + direction * radius).looking_at(focus, Vec3::Y)
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    studio: Res<Studio>,
    state: Res<AvatarLoadState>,
    mut preview: ResMut<AvatarPreview>,
    target: Res<PreviewImage>,
    mut images: ResMut<Assets<Image>>,
    parents: Query<&ChildOf>,
    meshes: Query<(Entity, &Aabb, &GlobalTransform), With<Mesh3d>>,
    mut cameras: Query<(&mut Camera, &mut Transform), With<PreviewCamera>>,
) {
    let Ok((mut camera, mut transform)) = cameras.single_mut() else {
        return;
    };
    let active =
        studio.mode == ViewMode::Studio && matches!(studio.tab, Tab::Characters | Tab::Scene);
    if camera.is_active != active {
        camera.is_active = active;
    }
    if !active {
        // The egui texture ID and camera target retain this image even while
        // hidden. Shrink the target once so its full-resolution GPU backing
        // can be released; the next active frame restores the requested size.
        if let Some(mut image) = images.get_mut(&target.0)
            && (image.width() != 1 || image.height() != 1)
        {
            image.resize(bevy::render::render_resource::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            });
            // Vec::resize alone retains the full previous allocation. The
            // hidden target is our RGBA8 scratch image, so discard its pixels.
            if image.data.is_some() {
                image.data = Some(vec![0; 4]);
            }
        }
        return;
    }
    if state.active != preview.root {
        preview.root = state.active;
        preview.bounds = None;
        preview.yaw = 0.0;
        preview.pitch = 0.0;
        preview.zoom = 1.0;
    }
    if preview.bounds.is_none()
        && let Some(root) = state.active
    {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (entity, aabb, global) in &meshes {
            if entity != root && !parents.iter_ancestors(entity).any(|p| p == root) {
                continue;
            }
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let point = global.transform_point(
                            Vec3::from(aabb.center)
                                + Vec3::from(aabb.half_extents) * Vec3::new(x, y, z),
                        );
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
        }
        if min.is_finite() && max.is_finite() {
            preview.bounds = Some((min, max));
        }
    }
    let size = preview.size;
    if let Some(mut image) = images.get_mut(&target.0)
        && (image.width() != size.x || image.height() != size.y)
    {
        image.resize(bevy::render::render_resource::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
    if let Some((min, max)) = preview.bounds {
        let (min, max) = preview.framing.bounds(min, max);
        *transform = frame(
            min,
            max,
            size.x as f32 / size.y as f32,
            preview.zoom,
            preview.yaw,
            preview.pitch,
        );
    }
}

/// Reserve a visible dock before creating the controls' independent scroll area.
/// Narrow windows keep a short, full-width live preview above the controls.
pub fn dock(ui: &mut egui::Ui, preview: &mut AvatarPreview) {
    let available = ui.available_size();
    if available.x >= 860.0 {
        egui::Panel::right("live_avatar_preview")
            .exact_size((available.x * 0.31).clamp(280.0, 520.0))
            .resizable(true)
            .show(ui, |ui| widget(ui, preview, ui.available_height()));
    } else {
        widget(ui, preview, (available.y * 0.32).clamp(140.0, 280.0));
        ui.separator();
    }
}
fn widget(ui: &mut egui::Ui, preview: &mut AvatarPreview, height: f32) {
    let top = ui.cursor().top();
    waifu_studio::settings_kit::row(
        ui,
        "Live preview",
        "Drag to rotate · scroll to zoom",
        |ui| {
            egui::ComboBox::from_id_salt("preview_framing")
                .width(180.0)
                .selected_text(match preview.framing {
                    Framing::FullBody => "Full body",
                    Framing::UpperBody => "Upper body",
                    Framing::Face => "Face & hair",
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        (Framing::FullBody, "Full body"),
                        (Framing::UpperBody, "Upper body"),
                        (Framing::Face, "Face & hair"),
                    ] {
                        if ui
                            .selectable_value(&mut preview.framing, value, label)
                            .changed()
                        {
                            preview.zoom = 1.0;
                        }
                    }
                });
        },
    );
    let size = egui::vec2(
        ui.available_width(),
        (height - (ui.cursor().top() - top) - 44.0).max(60.0),
    );
    if let Some(texture) = preview.texture {
        let response = ui.add(egui::Image::new((texture, size)).sense(egui::Sense::drag()));
        let pixels = ui.ctx().pixels_per_point();
        preview.size = target_size(size, pixels);
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            preview.yaw -= delta.x * 0.008;
            preview.pitch = (preview.pitch + delta.y * 0.008).clamp(-1.2, 1.2);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            preview.zoom = (preview.zoom * (scroll * 0.003).exp()).clamp(0.4, 4.0);
        }
    } else {
        ui.allocate_space(size);
    }
    if ui.button("Reset view").clicked() {
        preview.yaw = 0.0;
        preview.pitch = 0.0;
        preview.zoom = 1.0;
    }
}

fn target_size(size: egui::Vec2, pixels_per_point: f32) -> UVec2 {
    let pixels = Vec2::new(size.x, size.y) * pixels_per_point;
    // Scale both axes together: independently clamping them stretches the
    // rendered avatar when a narrow dock or a HiDPI view reaches a limit.
    let scale = (1536.0 / pixels.x).min(2048.0 / pixels.y).min(1.0);
    (pixels * scale).round().max(Vec2::ONE).as_uvec2()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn preview_fixture() -> (App, Entity, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("studio-preview-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(directory.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        studio.tab = Tab::Scene;
        studio.frame_requested = false;
        let mut app = App::new();
        app.insert_resource(studio)
            .init_resource::<AvatarLoadState>()
            .init_resource::<Assets<Image>>()
            .init_resource::<EguiUserTextures>()
            .add_plugins(AvatarPreviewPlugin);
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            ChildOf(root),
            Mesh3d::default(),
            Aabb::from_min_max(Vec3::new(-0.5, 0.0, -0.2), Vec3::new(0.5, 1.8, 0.2)),
            GlobalTransform::IDENTITY,
        ));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        app.update();
        let camera = app
            .world_mut()
            .query_filtered::<Entity, With<PreviewCamera>>()
            .single(app.world())
            .unwrap();
        (app, camera, directory)
    }

    fn widget_frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
        height: f32,
    ) -> egui::FullOutput {
        let mut preview = app.world_mut().resource_mut::<AvatarPreview>();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width + 40.0, height + 200.0),
                )),
                events,
                ..default()
            },
            |ui| {
                ui.set_width(width);
                widget(ui, &mut preview, height);
            },
        );
        output.textures_delta.clear();
        output
    }

    fn image_rect(app: &App, output: &egui::FullOutput) -> egui::Rect {
        fn locate(shape: &egui::Shape, texture: egui::TextureId) -> Option<egui::Rect> {
            match shape {
                // egui paints unrotated images as textured rectangles, and
                // rotated images as meshes. Either can be nested in a group.
                egui::Shape::Rect(rect)
                    if rect
                        .brush
                        .as_ref()
                        .is_some_and(|brush| brush.fill_texture_id == texture) =>
                {
                    Some(rect.rect)
                }
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh.calc_bounds()),
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| locate(shape, texture)),
                _ => None,
            }
        }
        let texture = app.world().resource::<AvatarPreview>().texture.unwrap();
        let rect = output
            .shapes
            .iter()
            .find_map(|shape| {
                let rect = locate(&shape.shape, texture)?;
                // Retain the complete widget bounds for aspect checks while
                // requiring that the actual image intersects the painted clip.
                rect.intersects(shape.clip_rect).then_some(rect)
            })
            .expect("preview texture must be painted within the visible clip");
        assert!(rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0);
        rect
    }

    fn click(position: egui::Pos2) -> Vec<egui::Event> {
        std::iter::once(egui::Event::PointerMoved(position))
            .chain(
                [true, false]
                    .into_iter()
                    .map(|pressed| egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: default(),
                    }),
            )
            .collect()
    }

    fn text_center(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
        fn locate(shape: &egui::Shape, text: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(label) if label.galley.job.text == text => {
                    Some(label.visual_bounding_rect().center())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| locate(shape, text)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| locate(&shape.shape, text))
            .unwrap_or_else(|| panic!("missing preview control: {text}"))
    }

    #[test]
    fn preview_widget_orbit_zoom_and_focus_survive_settings_round_trip() {
        let (mut app, camera, directory) = preview_fixture();
        let ctx = egui::Context::default();
        widget_frame(&mut app, &ctx, vec![], 300.0, 600.0);
        let output = widget_frame(&mut app, &ctx, vec![], 300.0, 600.0);
        let center = image_rect(&app, &output).center();
        widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(center)],
            300.0,
            600.0,
        );
        widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: default(),
            }],
            300.0,
            600.0,
        );
        let dragged = center + egui::vec2(35.0, 20.0);
        widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(dragged)],
            300.0,
            600.0,
        );
        widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: dragged,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: default(),
            }],
            300.0,
            600.0,
        );
        let output = widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 80.0),
                phase: egui::TouchPhase::Move,
                modifiers: default(),
            }],
            300.0,
            600.0,
        );
        let (yaw, pitch) = {
            let preview = app.world().resource::<AvatarPreview>();
            assert!(preview.yaw.abs() > 0.01 && preview.pitch.abs() > 0.01);
            assert!(preview.zoom > 1.0);
            (preview.yaw, preview.pitch)
        };
        let combo = text_center(&output, "Full body");
        // Move away from the image so inertial scrolling cannot affect its zoom.
        widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(combo)],
            300.0,
            600.0,
        );
        widget_frame(&mut app, &ctx, click(combo), 300.0, 600.0);
        assert!(
            egui::Popup::is_any_open(&ctx),
            "framing click must open the popup"
        );
        // Popup::show uses an invisible sizing pass on the opening frame.
        // Inspect the next painted frame, then click the actual option bounds.
        let popup = widget_frame(&mut app, &ctx, vec![], 300.0, 600.0);
        let face = text_center(&popup, "Face & hair");
        widget_frame(&mut app, &ctx, click(face), 300.0, 600.0);
        assert!(
            !egui::Popup::is_any_open(&ctx),
            "selecting a focus must close the popup"
        );
        let selected = widget_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(combo)],
            300.0,
            600.0,
        );
        text_center(&selected, "Face & hair");
        {
            let preview = app.world().resource::<AvatarPreview>();
            assert!(preview.framing == Framing::Face);
            assert_eq!(preview.zoom, 1.0);
            assert_eq!((preview.yaw, preview.pitch), (yaw, pitch));
        }
        app.update();
        let pose = *app.world().get::<Transform>(camera).unwrap();
        let focus = Vec3::new(0.0, 1.8 * 0.86, 0.0);
        assert!(
            (pose.rotation * Vec3::NEG_Z).abs_diff_eq((focus - pose.translation).normalize(), 1e-5)
        );
        for tab in [Tab::Chat, Tab::Connections, Tab::Characters, Tab::Scene] {
            app.world_mut().resource_mut::<Studio>().tab = tab;
            app.update();
            assert_eq!(
                app.world().get::<Camera>(camera).unwrap().is_active,
                matches!(tab, Tab::Characters | Tab::Scene)
            );
            assert_eq!(*app.world().get::<Transform>(camera).unwrap(), pose);
            let preview = app.world().resource::<AvatarPreview>();
            assert_eq!(
                (preview.yaw, preview.pitch, preview.zoom),
                (yaw, pitch, 1.0)
            );
            assert!(preview.framing == Framing::Face);
            assert!(!app.world().resource::<Studio>().frame_requested);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preview_widget_resize_preserves_aspect_and_orbit_at_texture_limits() {
        let (mut app, camera, directory) = preview_fixture();
        let ctx = egui::Context::default();
        {
            let mut preview = app.world_mut().resource_mut::<AvatarPreview>();
            preview.yaw = 0.4;
            preview.pitch = -0.2;
            preview.zoom = 1.7;
        }
        let initial = *app.world().get::<Transform>(camera).unwrap();
        for (width, height, pixels) in [
            (300.0, 80.0, 1.0),
            (2800.0, 600.0, 2.0),
            (300.0, 1800.0, 3.0),
            (600.0, 900.0, 1.0),
        ] {
            ctx.set_pixels_per_point(pixels);
            widget_frame(&mut app, &ctx, vec![], width, height);
            let output = widget_frame(&mut app, &ctx, vec![], width, height);
            let rect = image_rect(&app, &output);
            let size = app.world().resource::<AvatarPreview>().size;
            assert!(size.x > 0 && size.x <= 1536 && size.y > 0 && size.y <= 2048);
            // Allow rounding by one physical pixel without accepting stretching.
            assert!(
                (size.x as f32 * rect.height() - size.y as f32 * rect.width()).abs()
                    <= rect.width().max(rect.height())
            );
            app.update();
            let target = &app.world().resource::<PreviewImage>().0;
            let image = app.world().resource::<Assets<Image>>().get(target).unwrap();
            assert_eq!((image.width(), image.height()), (size.x, size.y));
            let preview = app.world().resource::<AvatarPreview>();
            assert_eq!((preview.yaw, preview.pitch, preview.zoom), (0.4, -0.2, 1.7));
        }
        assert_ne!(*app.world().get::<Transform>(camera).unwrap(), initial);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn hidden_preview_releases_target_and_restores_it_with_the_same_texture() {
        let (mut app, camera, directory) = preview_fixture();
        let texture = app.world().resource::<AvatarPreview>().texture;
        let target = app.world().resource::<PreviewImage>().0.clone();
        let dimensions = |app: &App| {
            let images = app.world().resource::<Assets<Image>>();
            let image = images.get(&target).unwrap();
            (image.width(), image.height())
        };
        assert_eq!(dimensions(&app), (600, 900));
        {
            let mut preview = app.world_mut().resource_mut::<AvatarPreview>();
            preview.yaw = 0.4;
            preview.pitch = -0.2;
            preview.zoom = 1.7;
        }
        app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        app.update();
        assert!(!app.world().get::<Camera>(camera).unwrap().is_active);
        assert_eq!(dimensions(&app), (1, 1));
        let images = app.world().resource::<Assets<Image>>();
        assert_eq!(
            images
                .get(&target)
                .unwrap()
                .data
                .as_ref()
                .unwrap()
                .capacity(),
            4
        );
        let inactive_tick = app
            .world()
            .entity(camera)
            .get_ref::<Camera>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(dimensions(&app), (1, 1));
        assert_eq!(
            app.world()
                .entity(camera)
                .get_ref::<Camera>()
                .unwrap()
                .last_changed(),
            inactive_tick
        );
        app.world_mut().resource_mut::<Studio>().mode = ViewMode::Studio;
        app.update();
        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
        assert_eq!(dimensions(&app), (600, 900));
        let preview = app.world().resource::<AvatarPreview>();
        assert_eq!(preview.texture, texture);
        assert_eq!((preview.yaw, preview.pitch, preview.zoom), (0.4, -0.2, 1.7));
        app.world_mut().resource_mut::<Studio>().tab = Tab::Chat;
        app.update();
        assert_eq!(dimensions(&app), (1, 1));
        app.world_mut().resource_mut::<Studio>().tab = Tab::Characters;
        app.update();
        assert_eq!(dimensions(&app), (600, 900));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn framing_contains_full_avatar_at_portrait_and_landscape_aspects() {
        let min = Vec3::new(-0.5, 0.0, -0.2);
        let max = Vec3::new(0.5, 1.8, 0.2);
        for aspect in [0.35, 0.67, 1.0, 2.5] {
            let pose = frame(min, max, aspect, 1.0, 0.0, 0.0);
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        let p = pose
                            .to_matrix()
                            .inverse()
                            .transform_point3(Vec3::new(x, y, z));
                        let half = (-p.z) * 17.5_f32.to_radians().tan();
                        assert!(p.z < 0.0 && p.y.abs() < half && p.x.abs() < half * aspect);
                    }
                }
            }
        }
    }
}
