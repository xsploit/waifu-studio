#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct DesktopOcclusion {
    rects: array<vec4<f32>, 16>,
    depths: array<vec4<f32>, 16>,
    counts: vec4<u32>,
};
@group(0) @binding(0) var<uniform> mask: DesktopOcclusion;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @builtin(frag_depth) f32 {
    var depth = 0.0;
    for (var i = 0u; i < mask.counts.x; i += 1u) {
        let rect = mask.rects[i];
        if all(in.uv >= rect.xy) && all(in.uv < rect.zw) {
            depth = max(depth, mask.depths[i].x);
        }
    }
    if depth == 0.0 { discard; }
    return depth;
}
