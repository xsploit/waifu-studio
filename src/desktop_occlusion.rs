//! Window ledges write depth before the avatar, without painting the transparent
//! desktop. Front-facing limbs remain visible; geometry behind the ledge does
//! not. This is desktop-window occlusion, independent of arm/body clearance.
use crate::{
    desktop_interaction::{ScreenRect, SurfaceId},
    desktop_platform::DesktopSnapshot,
};
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        Core3dSystems, FullscreenShader, core_3d::CORE_3D_DEPTH_FORMAT, schedule::Core3d,
    },
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_resource::{binding_types::uniform_buffer, *},
        renderer::{RenderAdapter, RenderContext, RenderDevice, ViewQuery},
        view::ViewDepthTexture,
    },
};
use std::time::{Duration, Instant};

const MAX_MASKS: usize = 16;

#[derive(Component, Clone, Copy, Default, ExtractComponent, ShaderType)]
pub struct DesktopOcclusion {
    rects: [Vec4; MAX_MASKS],
    depths: [Vec4; MAX_MASKS],
    counts: UVec4,
}

impl DesktopOcclusion {
    /// All geometry is derived from one fresh, logical-coordinate snapshot.
    /// Reverse Z: zero is far away, one is at the camera's near plane.
    pub fn for_seat(snapshot: &DesktopSnapshot, seat: SurfaceId, depth: f32, now: Instant) -> Self {
        let mut mask = Self::default();
        if !depth.is_finite()
            || !(0.0..1.0).contains(&depth)
            || now
                .checked_duration_since(snapshot.received_at)
                .is_none_or(|age| age > Duration::from_millis(500))
        {
            return mask;
        }
        let Some(own) = snapshot.own_window_rect.filter(|rect| valid_rect(*rect)) else {
            return mask;
        };
        let Some(support) = snapshot.surfaces.iter().find(|surface| surface.id == seat) else {
            return mask;
        };
        if !valid_rect(support.rect)
            || matches!(seat, SurfaceId::Window(id) if snapshot.own_window_ids.contains(&id))
        {
            return mask;
        }
        // A panel/floor seat is in front of the desktop. Treating its work
        // area as the backmost window made every ordinary app erase the avatar,
        // including her head. Only actual window ledges need depth occlusion.
        if !matches!(seat, SurfaceId::Window(_)) {
            return mask;
        }
        // The support is at pelvis depth. Windows above it in the desktop's
        // stacking order hide the whole avatar where their rectangles overlap.
        mask.push(own, support.rect, depth);
        for surface in &snapshot.surfaces {
            if surface.z_order < support.z_order
                && matches!(surface.id, SurfaceId::Window(id) if !snapshot.own_window_ids.contains(&id))
                && surface.id != seat
            {
                mask.push(own, surface.rect, 1.);
            }
        }
        mask
    }

    fn push(&mut self, own: ScreenRect, rect: ScreenRect, depth: f32) {
        if self.counts.x as usize == MAX_MASKS || !valid_rect(rect) {
            return;
        }
        let min = rect.origin.max(own.origin);
        let max = (rect.origin + rect.size).min(own.origin + own.size);
        if min.cmpge(max).any() {
            return;
        }
        let min = (min - own.origin) / own.size;
        let max = (max - own.origin) / own.size;
        let index = self.counts.x as usize;
        self.rects[index] = Vec4::new(min.x, min.y, max.x, max.y);
        self.depths[index].x = depth;
        self.counts.x += 1;
    }

    /// CPU equivalent of the GPU mask for geometry and regression checks.
    pub fn hides(&self, uv: Vec2, reverse_depth: f32) -> bool {
        (0..self.counts.x as usize).any(|i| {
            let r = self.rects[i];
            uv.x >= r.x
                && uv.y >= r.y
                && uv.x < r.z
                && uv.y < r.w
                && reverse_depth < self.depths[i].x
        })
    }
}

fn valid_rect(rect: ScreenRect) -> bool {
    rect.origin.is_finite()
        && rect.size.is_finite()
        && rect.size.min_element() > 0.
        && (rect.origin + rect.size).is_finite()
}

pub struct DesktopOcclusionPlugin;
impl Plugin for DesktopOcclusionPlugin {
    fn build(&self, app: &mut App) {
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        embedded_asset!(app, "desktop_occlusion.wgsl");
        app.add_plugins((
            ExtractComponentPlugin::<DesktopOcclusion>::default(),
            UniformComponentPlugin::<DesktopOcclusion>::default(),
        ));
        app.sub_app_mut(RenderApp)
            .add_systems(RenderStartup, prepare)
            // First access clears depth; all later passes load our mask. Run
            // before the ordinary prepass too, so it cannot restore hidden depth.
            .add_systems(Core3d, draw.before(Core3dSystems::Prepass));
    }
}

#[derive(Resource)]
struct OcclusionPipeline {
    layout: BindGroupLayoutDescriptor,
    variants: Vec<(u32, CachedRenderPipelineId)>,
}

fn prepare(
    mut commands: Commands,
    device: Res<RenderDevice>,
    adapter: Res<RenderAdapter>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "desktop_occlusion",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            uniform_buffer::<DesktopOcclusion>(true),
        ),
    );
    let shader = load_embedded_asset!(assets.as_ref(), "desktop_occlusion.wgsl");
    let variants = [1, 2, 4, 8]
        .into_iter()
        .filter(|samples| {
            // Pipeline creation validates sample support; query the actual device
            // format capabilities rather than requiring every MSAA variant.
            (*samples == 1
                || *samples == 4
                || device
                    .features()
                    .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES))
                && adapter
                    .get_texture_format_features(CORE_3D_DEPTH_FORMAT)
                    .flags
                    .sample_count_supported(*samples)
        })
        .map(|samples| {
            let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("desktop_window_depth".into()),
                layout: vec![layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    targets: vec![],
                    ..default()
                }),
                depth_stencil: Some(DepthStencilState {
                    format: CORE_3D_DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(CompareFunction::GreaterEqual),
                    stencil: default(),
                    bias: default(),
                }),
                multisample: MultisampleState {
                    count: samples,
                    ..default()
                },
                ..default()
            });
            (samples, id)
        })
        .collect();
    commands.insert_resource(OcclusionPipeline { layout, variants });
}

fn draw(
    view: ViewQuery<(
        &ViewDepthTexture,
        &DynamicUniformIndex<DesktopOcclusion>,
        &DesktopOcclusion,
    )>,
    pipeline: Option<Res<OcclusionPipeline>>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<DesktopOcclusion>>,
    mut ctx: RenderContext,
) {
    let (depth, index, mask) = view.into_inner();
    if mask.counts.x == 0 {
        return;
    }
    let Some(pipeline) = pipeline else {
        return;
    };
    let Some((_, id)) = pipeline
        .variants
        .iter()
        .find(|(samples, _)| *samples == depth.texture.sample_count())
    else {
        return;
    };
    let Some(render_pipeline) = cache.get_render_pipeline(*id) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "desktop_occlusion",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::single(binding),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("desktop_window_depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, &group, &[index.index()]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_interaction::DesktopSurface;

    fn snapshot() -> DesktopSnapshot {
        DesktopSnapshot {
            received_at: Instant::now(),
            surfaces: vec![
                DesktopSurface {
                    id: SurfaceId::Window(1),
                    rect: ScreenRect {
                        origin: Vec2::new(-100., 240.),
                        size: Vec2::new(600., 500.),
                    },
                    z_order: 2,
                },
                DesktopSurface {
                    id: SurfaceId::Window(2),
                    rect: ScreenRect {
                        origin: Vec2::new(-60., 160.),
                        size: Vec2::new(40., 240.),
                    },
                    z_order: 0,
                },
                DesktopSurface {
                    id: SurfaceId::Window(3),
                    rect: ScreenRect {
                        origin: Vec2::new(-100., 100.),
                        size: Vec2::splat(500.),
                    },
                    z_order: 3,
                },
                DesktopSurface {
                    id: SurfaceId::Window(9),
                    rect: ScreenRect {
                        origin: Vec2::new(-100., 100.),
                        size: Vec2::splat(500.),
                    },
                    z_order: 0,
                },
            ],
            own_window_ids: vec![9],
            own_window_rect: Some(ScreenRect {
                origin: Vec2::new(-100., 100.),
                size: Vec2::new(200., 400.),
            }),
            own_interactive_move: None,
            own_above_panels: None,
            cursor: Vec2::ZERO,
        }
    }

    #[test]
    fn seated_depth_preserves_front_legs_and_uses_real_stacking_and_negative_coordinates() {
        let snapshot = snapshot();
        let mask =
            DesktopOcclusion::for_seat(&snapshot, SurfaceId::Window(1), 0.3, snapshot.received_at);
        assert!(
            !mask.hides(Vec2::new(0.8, 0.2), 0.1),
            "upper body above ledge"
        );
        assert!(mask.hides(Vec2::new(0.8, 0.5), 0.2), "body behind support");
        assert!(
            !mask.hides(Vec2::new(0.8, 0.5), 0.4),
            "legs in front of support"
        );
        assert!(
            mask.hides(Vec2::new(0.3, 0.2), 0.9),
            "higher window covers avatar"
        );
        assert_eq!(mask.counts.x, 2, "own and lower windows do not mask");
    }

    #[test]
    fn stale_disconnected_missing_and_invalid_supports_clear_the_mask() {
        let mut snapshot = snapshot();
        for (seat, depth, time) in [
            (
                SurfaceId::Window(1),
                0.3,
                snapshot.received_at + Duration::from_secs(1),
            ),
            (SurfaceId::Window(404), 0.3, snapshot.received_at),
            (SurfaceId::Window(9), 0.3, snapshot.received_at),
            (SurfaceId::Window(1), f32::NAN, snapshot.received_at),
        ] {
            assert_eq!(
                DesktopOcclusion::for_seat(&snapshot, seat, depth, time)
                    .counts
                    .x,
                0
            );
        }
        snapshot.own_window_rect = None;
        assert_eq!(
            DesktopOcclusion::for_seat(&snapshot, SurfaceId::Window(1), 0.3, snapshot.received_at)
                .counts
                .x,
            0
        );
    }

    #[test]
    fn taskbar_seat_stays_in_front_of_panel_and_overlapping_applications() {
        let mut snapshot = snapshot();
        snapshot.surfaces.push(DesktopSurface {
            id: SurfaceId::WorkArea(7),
            rect: ScreenRect {
                origin: Vec2::new(-100., 0.),
                size: Vec2::new(600., 400.),
            },
            z_order: i32::MAX,
        });
        let mask = DesktopOcclusion::for_seat(
            &snapshot,
            SurfaceId::WorkArea(7),
            0.3,
            snapshot.received_at,
        );
        assert!(!mask.hides(Vec2::new(0.5, 0.7), 0.1));
        assert!(!mask.hides(Vec2::new(0.5, 0.8), 0.1));
        assert!(!mask.hides(Vec2::new(0.5, 0.8), 0.4));
        assert_eq!(
            mask.counts.x, 0,
            "ordinary windows must not erase a taskbar companion"
        );
    }
}
