//! Optional linear RGB correction before tonemapping, following WW4's
//! ColorCorrectionShader: gain * pow(max(rgb, 0), power). Alpha is preserved.
//! Render integration follows Bevy 0.19's custom_post_processing example
//! (Bevy contributors, MIT OR Apache-2.0).
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d, tonemapping::tonemapping},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_resource::{
            binding_types::{sampler, texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        view::ViewTarget,
    },
};

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub(crate) struct ColorCorrection {
    pub power: Vec4,
    pub gain: Vec4,
}

pub(crate) struct ColorCorrectionPlugin;
impl Plugin for ColorCorrectionPlugin {
    fn build(&self, app: &mut App) {
        // CPU-only test apps intentionally omit both assets and the renderer.
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        embedded_asset!(app, "color_correction.wgsl");
        app.add_plugins((
            ExtractComponentPlugin::<ColorCorrection>::default(),
            UniformComponentPlugin::<ColorCorrection>::default(),
        ));
        let render = app.sub_app_mut(RenderApp);
        render.add_systems(RenderStartup, prepare);
        render.add_systems(
            Core3d,
            apply.in_set(Core3dSystems::PostProcess).before(tonemapping),
        );
    }
}

#[derive(Resource)]
struct CorrectionPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    pipeline: CachedRenderPipelineId,
}

fn prepare(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "rgb_correction",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ColorCorrection>(true),
            ),
        ),
    );
    let shader = load_embedded_asset!(assets.as_ref(), "color_correction.wgsl");
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("linear_rgb_correction".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader,
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });
    commands.insert_resource(CorrectionPipeline {
        layout,
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        pipeline,
    });
}

fn apply(
    view: ViewQuery<(&ViewTarget, &DynamicUniformIndex<ColorCorrection>)>,
    pipeline: Option<Res<CorrectionPipeline>>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<ColorCorrection>>,
    mut ctx: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let (target, index) = view.into_inner();
    // VisualPlugin attaches correction only to its HDR cameras. Never flip an
    // unsupported target or a texture whose shader is not ready.
    if target.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }
    let Some(render_pipeline) = cache.get_render_pipeline(pipeline.pipeline) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let post = target.post_process_write();
    // Rebind each frame: the uniform buffer may be replaced after camera count
    // changes, even if the alternating source texture's ID remains unchanged.
    let group = ctx.render_device().create_bind_group(
        "rgb_correction",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((post.source, &pipeline.sampler, binding)),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("linear_rgb_correction"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, &group, &[index.index()]);
    pass.draw(0..3, 0..1);
}
