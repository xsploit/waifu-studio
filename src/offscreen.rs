//! Windowless capture of the actual studio UI for layout/render verification.
use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::render_resource::{TextureFormat, TextureUsages},
};
use bevy_egui::PrimaryEguiContext;

#[derive(Resource)]
pub(crate) struct OffscreenCapture(pub UVec2, pub bool);

pub(crate) fn setup(
    mut commands: Commands,
    options: Res<OffscreenCapture>,
    mut images: ResMut<Assets<Image>>,
    camera: Single<Entity, With<PrimaryEguiContext>>,
    mut capture: ResMut<crate::Capture>,
) {
    let mut image = Image::new_target_texture(
        options.0.x,
        options.0.y,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = images.add(image);
    commands.entity(*camera).insert((
        RenderTarget::Image(target.clone().into()),
        bevy::camera::ShadowLodOrigin,
    ));
    capture.target = Some(target);
}
