// Composites the progressive edge accumulation buffer into the graph view - see
// `Accumulation` in `graph.rs`.
//
// The edges are drawn by their own camera into an offscreen Rgba16Float image that is
// only cleared when the view changes, its blit adding into the buffer rather than
// replacing it, so each frame's deposit lands on top of its predecessors. This
// pass is what turns that running sum back into a picture: after `n` deposits the buffer
// holds `n` times the expected full-graph image (a chunk drawn one frame in P is drawn at
// P times the brightness, whatever P is - see `edge_material`), so scaling by `1/n` is
// correct at every n, and only the grain changes as n climbs.
//
// Drawn as a single quad whose vertices already sit at +/-1, so the vertex stage ignores
// the view entirely and passes clip space straight through. `z = 1` is the *near* plane
// under bevy's reverse-Z depth, which is what keeps the composite from being depth-
// rejected by the node spheres - additive light has no depth to speak of anyway.

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> gain: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var accumulation: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var accumulation_sampler: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
}

struct Output {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> Output {
    var out: Output;
    out.clip_position = vec4(vertex.position.xy, 1.0, 1.0);
    // clip space is y-up and texture space is y-down, so v is flipped rather than derived
    out.uv = vec2(vertex.position.x * 0.5 + 0.5, 0.5 - vertex.position.y * 0.5);
    return out;
}

// Alpha stays 0: `AlphaMode::Add` is bevy's premultiplied blend, so the colour is added
// and the destination is scaled by `1 - alpha`. Anything but 0 would dim the nodes and
// the background underneath - the same trick `GraphMaterial::additive` already relies on.
@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let accumulated = textureSample(accumulation, accumulation_sampler, in.uv);
    return vec4(accumulated.rgb * gain.x, 0.0);
}
