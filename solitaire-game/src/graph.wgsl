// Unlit material for the constellation graph scene - see `graph.rs`.
//
// This replaces both stages of the PBR pipeline rather than extending it: the graph is
// 1.68M node spheres plus millions of edges, and wants none of what PBR does per
// fragment. What is left is the position transform and a flat colour.
//
// Deliberately does no thinning of its own. An earlier version discarded edge fragments
// by distance, and measurement killed it: cutting *every* edge fragment - so that the
// blend write never happened at all - only took the worst viewpoint from ~5 to ~10 fps,
// because a discarded fragment still costs its vertex shading, primitive setup, clipping
// and rasterization. The edge pass is bound by primitive count, not fill, so the thinning
// that survived is `decimate` in graph.rs, which drops edges before they are ever put in
// a mesh.
//
// Only `@location(0)` is declared, so the meshes need no normals and no uvs - a vertex
// buffer layout may hand over attributes the shader ignores, but every attribute the
// shader reads has to be there.

#import bevy_pbr::{
    mesh_functions::{get_world_from_local, mesh_position_local_to_world},
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

@vertex
fn vertex(vertex: Vertex) -> @builtin(position) vec4<f32> {
    let world_from_local = get_world_from_local(vertex.instance_index);
    let world_position = mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
    return position_world_to_clip(world_position.xyz);
}

@fragment
fn fragment() -> @location(0) vec4<f32> {
    return color;
}
