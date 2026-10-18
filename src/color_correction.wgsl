#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct Correction {
    power: vec4<f32>,
    gain: vec4<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> correction: Correction;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let sampled = textureSample(source, source_sampler, in.uv);
    let rgb = correction.gain.xyz * pow(max(sampled.rgb, vec3<f32>(0.0)), correction.power.xyz);
    return vec4<f32>(rgb, sampled.a);
}
