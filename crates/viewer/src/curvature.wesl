// Display curvature (render only): every vertex is lowered by k·d², with k = 1 / (2 R_vis) and d
// the horizontal distance to the camera. World positions are already camera-relative (render
// origin + torus `delta`), so `d` is a plain difference of small coordinates.
//
// Normals follow the same mapping (inverse transpose of its Jacobian), so the lighting matches
// the displayed shape.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
};

// x: k = 1 / (2 R_vis), 0 when flat. y, z, w: unused.
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> curvature: vec4<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);

    let k = curvature.x;
    var world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    let d = world.xz - view.world_position.xz;
    world.y -= k * dot(d, d);
    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);

@if(VERTEX_NORMALS) {
    let n = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
    out.world_normal = normalize(vec3<f32>(n.x + 2.0 * k * d.x * n.y, n.y, n.z + 2.0 * k * d.y * n.y));
}

@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_COLORS)
    out.color = v.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_in.instance_index, world_from_local[3]);

    return out;
}
