// Unlit material for the constellation graph scene - see `graph.rs`.
//
// This replaces both stages of the PBR pipeline rather than extending it: the graph is
// 129k node spheres plus every legal move between them, and wants none of what PBR does
// per fragment. What is left is the position transform, a flat colour, and the camera's
// distance fog, which is the scene's main depth cue and the one thing PBR was doing here
// that is worth keeping.
//
// Only `@location(0)` is declared, so the meshes need no normals and no uvs - a vertex
// buffer layout may hand over attributes the shader ignores, but every attribute the
// shader reads has to be there.

#import bevy_pbr::{
    mesh_functions::{get_world_from_local, mesh_position_local_to_world},
    mesh_view_bindings::{view, fog},
    view_transformations::position_world_to_clip,
}

// The literal fragment output rather than a base colour - `GraphMaterial::color` is
// premultiplied on the cpu side, which is what lets one shader with no branch and no
// shader def serve both the opaque nodes and the additive edges. See `GraphMaterial`.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> color: vec4<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    let world_from_local = get_world_from_local(vertex.instance_index);
    let world_position = mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));

    var out: VertexOutput;
    out.world_position = world_position.xyz;
    out.clip_position = position_world_to_clip(world_position.xyz);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var out = color;

#ifdef DISTANCE_FOG
    // The graph camera's fog is always `FogFalloff::Linear`, so this ramp is the whole
    // of it, rather than the mode dispatch in `bevy_pbr::pbr_functions::apply_fog`.
    // `be.x`/`be.y` are where the linear falloff's start/end live in the fog uniform.
    let distance = length(in.world_position - view.world_position);
    let ramp = clamp((distance - fog.be.x) / (fog.be.y - fog.be.x), 0.0, 1.0);
    let fog_factor = ramp * fog.base_color.a;

    // Fogging in premultiplied-alpha space is compositing an opaque, fog-coloured layer
    // *under* the fragment, which is why this one line is right for both blend modes:
    // the opaque nodes (a = 1) mix towards the fog colour, and the additive edges
    // (a = 0) fade towards nothing, which is what "the background" means when the
    // blend is `src + dst * (1 - src.a)`. Mixing them towards a grey instead - what
    // `apply_fog` does, since it preserves the input alpha - would make the far edges
    // brighter with distance rather than dimmer.
    out = mix(out, vec4(fog.base_color.rgb, 1.0) * out.a, fog_factor);
#endif

    return out;
}
