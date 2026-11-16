// Display curvature and per-pixel sun (render only).
//
// Vertex: every vertex is lowered by k·d², with k = 1 / (2 R_vis) and d the horizontal distance
// to the camera, whose render-space position comes from the uniform (the shadow pass has the
// light as its view). Normals stay the flat-world normals: lighting happens in the flat world,
// where the sun direction depends on the position (§2.4).
//
// Fragment: the sun is computed per pixel from the fragment's world position (latitude from z,
// hour angle from x, δ_eff and the seam band from the seam distance), so the terminator, the
// seam twilight band and distant sunsets show on the horizon. Direct light fades in from 0° to
// +10° of e_vis, sky light from −6° to +10° (§2.5). Shadows come from Bevy's cascades of the
// camera's sun (the sun barely moves over the shadow range). Clouds (max of c_min(s) and the
// weather) dim the direct light. Seasonal snow covers every face but the downward ones (the
// sides of the voxel steps match their tops); the snow cover of the 160 m climate cell is
// corrected by the lapse rate between the fragment's height and the cell's mean height, so
// the snow line follows the fine relief.

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
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> curved: Curved;
// r: weather cloud cover, g: snow cover, b: temperature ((T + 50 °C) / 100), a: mean height of
// the climate cell (/ 3 500 m), over the whole world (repeat sampler).
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var climate_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var climate_smp: sampler;

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const HALF_PI: f32 = 1.5707964;
const OBLIQUITY: f32 = 0.40910518;
const SEAM_MARGIN: f32 = 0.06981317; // 4°
const LIGHT_FADE: f32 = 0.17453292; // 10°
const SKY_LOW: f32 = -0.10471976; // −6°
const CLIMATE_HEIGHT: f32 = 3500.0;

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

struct LocalSun {
    // Towards the visible sun, world axes (x east, y up, z south).
    dir: vec3<f32>,
    e_vis: f32,
    // Seam distance, m.
    s: f32,
}

// Sun at a point `dx` metres east of the camera, at wrapped world z (spec §2.4, §2.5).
fn local_sun(dx: f32, z_world: f32) -> LocalSun {
    let z = z_world - floor(z_world / WORLD) * WORLD;
    let phi = asin(clamp((50000.0 - z) / 50000.0, -1.0, 1.0));
    let s = min(z, WORLD - z);
    let w = smoothstep(0.0, curved.seam.x, s);
    let dec = curved.sun.w * w;
    let h = curved.sun.z + TAU * dx / WORLD;
    let e = asin(clamp(sin(phi) * sin(dec) + cos(phi) * cos(dec) * cos(h), -1.0, 1.0));
    let a = atan2(-cos(dec) * sin(h), sin(dec) * cos(phi) - cos(dec) * cos(h) * sin(phi));
    let r = 1.0 - smoothstep(curved.seam.y, curved.seam.z, s);
    let e_vis = e - r * (HALF_PI - abs(phi) + OBLIQUITY * w + SEAM_MARGIN);
    let c = cos(e_vis);
    return LocalSun(vec3<f32>(c * sin(a), sin(e_vis), -c * cos(a)), e_vis, s);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Water is tagged by a vertex alpha below 1 (chunk water is translucent, horizon water is
    // opaque but tagged); read it before alpha_discard forces opaque alphas to 1.
    var tag = 1.0;
@if(VERTEX_COLORS)
    tag = in.color.a;
    let is_water = tag < 0.99;
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );
    var albedo = pbr_input.material.base_color.rgb;
    var alpha = pbr_input.material.base_color.a;
    let n = pbr_input.N;
    // Chunk water is translucent but its sea floor is often not loaded: darken it towards the
    // deep-water tone of the horizon and keep it nearly opaque (depth-aware water: M6).
    if is_water && alpha < 0.99 {
        albedo *= 0.5;
        alpha = max(alpha, 0.9);
    }

    // World position of the fragment (camera-relative offsets are exact across the seam).
    let rel = in.world_position.xz - curved.drop.yw;
    let wx = curved.sun.x + rel.x;
    let wz = curved.sun.y + rel.y;
    let sun = local_sun(rel.x, wz);

    let climate = textureSample(climate_tex, climate_smp, vec2<f32>(wx, wz) / WORLD);
    let c_min = 1.0 - smoothstep(curved.seam.z, curved.seam.w, sun.s);
    let cloud = max(c_min, climate.r);

    // Seasonal snow on land faces (not on water), sides included, never on downward faces.
    if !is_water {
        let h = in.world_position.y + curved.drop.x * dot(rel, rel) - curved.drop.z + curved.snow.x;
        let facing = smoothstep(-0.5, -0.2, n.y);
        let snow = smoothstep(0.15, 0.6, snow_cover(climate, h)) * facing;
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

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(color, alpha));
    return out;
}
