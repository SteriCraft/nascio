// Display curvature and per-pixel sun (render only).
//
// Vertex: every vertex is lowered by k·d², with k = 1 / (2 R_vis) and d the horizontal distance
// to the camera, whose render-space position comes from the uniform (the shadow pass has the
// light as its view). Normals stay the flat-world normals: lighting happens in the flat world,
// where the sun direction depends on the position (§2.4).
//
// Fragment: the sun (`sun.wesl`) is computed per pixel from the fragment's world position
// (latitude from z, hour angle from x, δ_eff and the seam band from the seam distance), so the
// terminator, the seam twilight band and distant sunsets show on the horizon. Direct light
// fades in from 0° to +10° of e_vis, sky light from −6° to +10° (§2.5). Shadows come from Bevy's cascades of the
// camera's sun (the sun barely moves over the shadow range). Clouds (max of c_min(s) and the
// weather) dim the direct light. Seasonal snow covers every face but the downward ones (the
// sides of the voxel steps match their tops); the snow cover of the 160 m climate cell is
// corrected by the lapse rate between the fragment's height and the cell's mean height, so
// the snow line follows the fine relief. Haze and the far edge of the relief use the same air
// as the sky dome (`atmosphere` below), so they match it at every height (pre-M6).

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    mesh_view_bindings::{view, lights},
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    view_transformations::{position_world_to_clip, position_world_to_view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, main_pass_post_lighting_processing},
    shadows,
};
import nascio_viewer::atmosphere::{Air, air_haze, atmosphere};
import nascio_viewer::sun::{LocalSun, local_sun};

struct Curved {
    // x: k = 1 / (2 R_vis), 0 when flat; y, z, w: camera position in render space.
    drop: vec4<f32>,
    // x, z: camera world position (m, wrapped); z: hour angle at the camera (rad);
    // w: declination δ(N) (rad).
    sun: vec4<f32>,
    // s_fade, s_dark, s_light, s_clear (m).
    seam: vec4<f32>,
    // x: shadows (0/1); y: direct light scale; z: sky light scale; w: night floor.
    light: vec4<f32>,
    // x: camera world height (m); y: lapse rate (°C/m); z: snow cover per °C; w: unused.
    snow: vec4<f32>,
    // xyz: towards the visible sun at the camera; w: e_vis at the camera (rad).
    air: vec4<f32>,
    // x: cloud cover at the camera; y, z: horizontal distances where the relief starts and
    // ends fading into the sky (m); w: forced weather cloud cover, < 0 when not forced.
    haze: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> curved: Curved;
// r: weather cloud cover, g: snow cover, b: temperature ((T + 50 °C) / 100), a: mean height of
// the climate cell (/ 3 500 m), over the whole world (repeat sampler).
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var climate_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var climate_smp: sampler;
// Colour of the world's surface (repeat sampler), for the ground beyond the relief.
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var ground_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var ground_smp: sampler;

const WORLD: f32 = 100000.0;
const LIGHT_FADE: f32 = 0.17453292; // 10°
const SKY_LOW: f32 = -0.10471976; // −6°
const CLIMATE_HEIGHT: f32 = 3500.0;
// Kinds of the material channel: `nascio_voxel::mesh::MATERIAL_*` (checked by a test).
const MATERIAL_GROUND: f32 = 0.0;
const MATERIAL_WATER: f32 = 1.0;
const MATERIAL_FOLIAGE: f32 = 2.0;


// Snow cover at a fragment of world height `h`: the cell's cover, less snow below the cell's
// mean height, more above it where it is cold enough (no summer snow on warm peaks).
fn snow_cover(climate: vec4<f32>, h: f32) -> f32 {
    let t_cell = climate.b * 100.0 - 50.0;
    let dt = -curved.snow.y * (h - climate.a * CLIMATE_HEIGHT);
    let t_local = t_cell + dt;
    let k = curved.snow.z;
    if dt > 0.0 {
        return clamp(climate.g - k * dt, 0.0, 1.0);
    }
    let cold = 1.0 - smoothstep(0.0, 5.0, t_local);
    return clamp(climate.g - k * dt * cold, 0.0, 1.0);
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);

    var world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    let d = world.xz - curved.drop.yw;
    world.y -= curved.drop.x * dot(d, d);
    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);

@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);

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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Material channel (UV0, `nascio_voxel::mesh::MeshData::material`): kind and albedo factor
    // of water. Meshes without it are ground.
    var material = vec2<f32>(MATERIAL_GROUND, 1.0);
@if(VERTEX_UVS_A)
    material = in.uv;
    let is_water = abs(material.x - MATERIAL_WATER) < 0.5;
    // Foliage: its sides keep their green under the snow.
    let is_foliage = abs(material.x - MATERIAL_FOLIAGE) < 0.5;
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );
    var albedo = pbr_input.material.base_color.rgb;
    var alpha = pbr_input.material.base_color.a;
    let n = pbr_input.N;
    // Water darkens with the depth of its column (`nascio_voxel::mesh::water_shade`).
    if is_water {
        albedo *= material.y;
    }

    // World position of the fragment (camera-relative offsets are exact across the seam).
    let rel = in.world_position.xz - curved.drop.yw;
    let wx = curved.sun.x + rel.x;
    let wz = curved.sun.y + rel.y;
    let sun = local_sun(rel.x, wz, curved.sun.z, curved.sun.w, curved.seam);

    let climate = textureSample(climate_tex, climate_smp, vec2<f32>(wx, wz) / WORLD);
    let c_min = 1.0 - smoothstep(curved.seam.z, curved.seam.w, sun.s);
    // `haze.w` >= 0 forces the weather cloud cover (captures).
    let weather = select(climate.r, curved.haze.w, curved.haze.w >= 0.0);
    let cloud = max(c_min, weather);

    // Seasonal snow on land faces (not on water), sides included, never on downward faces.
    // World height of the fragment (the display drop undone), for the snow and the haze.
    let hf = in.world_position.y + curved.drop.x * dot(rel, rel) - curved.drop.z + curved.snow.x;
    if !is_water {
        let facing = select(
            smoothstep(-0.5, -0.2, n.y),
            0.05 + 0.85 * smoothstep(0.5, 0.9, n.y),
            is_foliage
        );
        let snow = smoothstep(0.15, 0.6, snow_cover(climate, hf)) * facing;
        albedo = mix(albedo, vec3<f32>(0.86, 0.89, 0.93), snow);
    }

    let direct = smoothstep(0.0, LIGHT_FADE, sun.e_vis) * curved.light.y * (1.0 - 0.8 * cloud);
    let sky = smoothstep(SKY_LOW, LIGHT_FADE, sun.e_vis);
    let warm = smoothstep(0.0, 0.35, sun.e_vis);
    let sun_color = mix(vec3<f32>(1.0, 0.6, 0.35), vec3<f32>(1.0, 0.96, 0.9), warm);
    let sky_color = mix(vec3<f32>(0.42, 0.47, 0.56), vec3<f32>(0.5, 0.52, 0.55), cloud);
    let night = vec3<f32>(0.035, 0.045, 0.08) * curved.light.w;
    var ambient = night + sky_color * sky * curved.light.z;

    var shadow = 1.0;
    if curved.light.x > 0.5 && lights.n_directional_lights > 0u {
        let light = lights.directional_lights[0];
        if (light.flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            let view_z = position_world_to_view(in.world_position.xyz).z;
            shadow = shadows::fetch_directional_shadow(0u, in.world_position, pbr_input.world_normal, view_z, in.position.xy);
        }
    }
    let lambert = max(dot(n, sun.dir), 0.0);
    var color = albedo * (ambient + sun_color * direct * lambert * shadow);

    // Water: sun glint and a little sky reflection.
    if is_water {
        let r = reflect(-sun.dir, n);
        let glint = pow(max(dot(r, pbr_input.V), 0.0), 80.0) * direct * shadow;
        let fresnel = pow(1.0 - max(dot(n, pbr_input.V), 0.0), 4.0);
        color += sun_color * glint * 2.0 + sky_color * sky * fresnel * 0.3;
    }

    // Haze, then the far edge of the relief fades into the sky dome: the same air (no seam
    // between the relief, the curvature of the planet and the sky).
    let to_frag = in.world_position.xyz - curved.drop.yzw;
    let len = length(to_frag);
    let d = to_frag / max(len, 1e-3);
    let air = Air(curved.air.xyz, curved.air.w, curved.haze.x, curved.snow.x, curved.drop.x, curved.sun.x, curved.sun.y);
    color = air_haze(air, d, len, hf, color);
    let edge = smoothstep(curved.haze.y, curved.haze.z, length(rel));
    if edge > 0.0 {
        color = mix(color, atmosphere(air, d, ground_tex, ground_smp), edge);
    }

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(color, alpha));
    return out;
}
