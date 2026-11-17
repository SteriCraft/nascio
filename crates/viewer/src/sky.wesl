// Sky dome (spec §2.5 – §2.8), drawn on a sphere around the camera.
//
// - Colour from the visible sun elevation at the camera: day, twilight glow towards the sun,
//   night; overcast grey under the cloud cover c = max(c_min(s), weather).
// - Sun and Moon discs (the Moon lit from the geometric sun: its phase), weighted by 1 − c.
//   The Moon is textured (NASA SVS CGI Moon Kit): near side towards the observer, lunar north
//   towards the celestial pole.
// - Aurorae: curtains on the geographic ring around the seam band (§2.7), at a few kilometres
//   of altitude. The view ray meets the displayed (lowered) shell `y = H − k·d²`, so the ring
//   is seen far away, low on the horizon. Night only (e_vis at the curtain), masked by clouds.
//   The stars are a separate mesh.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
// Same tone mapping as the lit materials (the relief fades into this dome).
@if(TONEMAP_IN_SHADER) import bevy_core_pipeline::tonemapping::tone_mapping;
import nascio_viewer::atmosphere::{Air, air_below_limb, air_cloud, air_density, atmosphere};
import nascio_viewer::sun::local_sun;

struct Sky {
    // xyz: towards the visible sun at the camera; w: e_vis at the camera (rad).
    sun: vec4<f32>,
    // xyz: towards the Moon; w: unused.
    moon: vec4<f32>,
    // xyz: towards the geometric sun (Moon phase); w: cloud cover at the camera.
    sun_geo: vec4<f32>,
    // x, y: camera world x, z (m); z: camera height (m); w: k = 1 / (2 R_vis).
    cam: vec4<f32>,
    // x: hour angle at the camera; y: declination δ(N); z: animation time (s); w: aurora gain.
    time: vec4<f32>,
    // s_fade, s_dark, s_light, s_clear (m).
    seam: vec4<f32>,
    // x: ring peak s_aurora; y: ring outer edge; z: curtain base altitude (m); w: unused.
    aurora: vec4<f32>,
    // xyz: towards the north celestial pole; w: unused.
    pole: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sky: Sky;
// Moon albedo map, equirectangular, longitude 0 in the middle, east to the right.
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var moon_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var moon_smp: sampler;
// Colour of the world's surface (repeat sampler), for the ground beyond the relief.
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var ground_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var ground_smp: sampler;

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const PI: f32 = 3.1415927;
const DEG: f32 = 0.017453292;


// Visible sun elevation at a point `dx` m east of the camera, at world z (§2.4, §2.5).
fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.547);
}

// Value noise, periodic in x with period `px` lattice cells (the torus wraps every 100 km).
fn noise(p: vec2<f32>, px: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x0 = i.x - floor(i.x / px) * px;
    let x1 = select(x0 + 1.0, 0.0, x0 + 1.0 >= px);
    let a = hash(vec2<f32>(x0, i.y));
    let b = hash(vec2<f32>(x1, i.y));
    let c = hash(vec2<f32>(x0, i.y + 1.0));
    let e = hash(vec2<f32>(x1, i.y + 1.0));
    return mix(mix(a, b, u.x), mix(c, e, u.x), u.y);
}

// Aurora emission along the view direction `d`.
fn aurora(d: vec3<f32>) -> vec3<f32> {
    let k = sky.cam.w;
    let horiz = length(d.xz);
    var sum = vec3<f32>(0.0);
    let layers = 10;
    // Per-pixel jitter of the shells: turns the banding between them into fine noise.
    let jitter = hash(d.xy * 4096.0 + d.zx * 977.0);
    for (var layer = 0; layer < layers; layer++) {
        let h01 = (f32(layer) + jitter) / f32(layers);
        let alt = sky.aurora.z + h01 * 4000.0;
        let c0 = sky.cam.z - alt;
        if c0 >= 0.0 {
            continue;
        }
        let a = k * horiz * horiz;
        var t: f32;
        if a > 1e-12 {
            t = (-d.y + sqrt(d.y * d.y - 4.0 * a * c0)) / (2.0 * a);
        } else if d.y > 1e-4 {
            t = -c0 / d.y;
        } else {
            continue;
        }
        let dist = t * horiz;
        if dist > 45000.0 {
            continue;
        }
        let rel = d.xz * t;
        let wx = sky.cam.x + rel.x;
        let wz = sky.cam.y + rel.y;
        let zz = wz - floor(wz / WORLD) * WORLD;
        let s = min(zz, WORLD - zz);
        let ring = smoothstep(sky.seam.w, sky.aurora.x, s) * (1.0 - smoothstep(sky.aurora.x, sky.aurora.y, s));
        if ring <= 0.0 {
            continue;
        }
        let night = 1.0 - smoothstep(-12.0 * DEG, -6.0 * DEG, local_sun(rel.x, wz, sky.time.x, sky.time.y, sky.seam).e_vis);
        // Curtains: vertical sheets, so the pattern depends on the horizontal position only
        // (folds along the ring, drifting slowly); bright lower edge, purple tops.
        let tt = sky.time.z * 0.03;
        let fold = noise(vec2<f32>(wx / 1250.0 + tt, s / 700.0), 80.0)
            * noise(vec2<f32>(wx / 200.0 - tt * 2.0, s / 260.0), 500.0);
        let sheet = smoothstep(0.18, 0.5, fold);
        let fade = exp(-dist / 30000.0);
        let profile = (1.0 - h01) * (1.0 - h01) + 0.15;
        let col = mix(vec3<f32>(0.2, 1.0, 0.5), vec3<f32>(0.6, 0.25, 0.85), smoothstep(0.35, 1.0, h01));
        sum += col * ring * night * sheet * fade * profile;
    }
    return sum * sky.time.w * 0.35;
}

fn finish(col: vec3<f32>) -> vec4<f32> {
    var out = vec4<f32>(col, 1.0);
@if(TONEMAP_IN_SHADER) {
    out = tone_mapping(out, view.color_grading);
}
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = normalize(in.world_position.xyz - view.world_position.xyz);
    let e = sky.sun.w;
    let air = Air(sky.sun.xyz, e, sky.sun_geo.w, sky.cam.z, sky.cam.w, sky.cam.x, sky.cam.y);
    let day_k = smoothstep(-2.0 * DEG, 15.0 * DEG, e);
    var col = atmosphere(air, d, ground_tex, ground_smp);
    // The planet hides the sun, the Moon and the aurorae below the limb.
    if air_below_limb(air, d) {
        return finish(col);
    }
    let clear = 1.0 - air_cloud(air);

    // Sun disc and halo.
    let cs = dot(d, sky.sun.xyz);
    let disc = smoothstep(cos(0.7 * DEG), cos(0.5 * DEG), cs);
    // The wide halo is scattered by the air: it fades with height.
    let halo = pow(max(cs, 0.0), 300.0) * 0.6
        + pow(max(cs, 0.0), 12.0) * 0.08 * day_k * air_density(air.h);
    let sun_col = mix(vec3<f32>(1.0, 0.55, 0.3), vec3<f32>(1.0, 0.97, 0.9), smoothstep(0.0, 12.0 * DEG, e));
    col += sun_col * (disc * 12.0 + halo) * clear * step(-1.0 * DEG, e);

    // Moon disc with its phase.
    let m = sky.moon.xyz;
    let cm = dot(d, m);
    let r_moon = 1.1 * DEG;
    if cm > cos(r_moon) {
        let off = (d - m * cm) / sin(r_moon);
        let nz = sqrt(max(1.0 - dot(off, off), 0.0));
        // The visible hemisphere faces the observer (−m).
        let normal = normalize(off - m * nz);
        let lit = max(dot(normal, sky.sun_geo.xyz), 0.0);
        let edge = smoothstep(cos(r_moon), cos(r_moon * 0.92), cm);
        // Selenographic coordinates: the sub-observer point (−m) is (0, 0), lunar north is
        // the celestial pole projected on the disc, lunar east points to the sky's west.
        var up = sky.pole.xyz - m * dot(sky.pole.xyz, m);
        if dot(up, up) < 1e-6 {
            up = vec3<f32>(0.0, 1.0, 0.0) - m * m.y;
        }
        up = normalize(up);
        let east = cross(m, up);
        let lat = asin(clamp(dot(normal, up), -1.0, 1.0));
        let lon = atan2(dot(normal, east), dot(normal, -m));
        let uv = vec2<f32>(0.5 + lon / TAU, 0.5 - lat / PI);
        let albedo = textureSampleLevel(moon_tex, moon_smp, uv, 0.0).rgb;
        let moon = albedo * 1.4 * (0.03 + 1.2 * lit);
        col = mix(col, col + moon, edge * clear);
    }

    // Aurorae are lost in a bright sky: only twilight and night skies show them.
    col += aurora(d) * clear * (1.0 - 0.95 * day_k * air_density(air.h));
    return finish(col);
}
