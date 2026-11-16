// Sky dome (spec §2.5 – §2.8), drawn on a sphere around the camera.
//
// - Colour from the visible sun elevation at the camera: day, twilight glow towards the sun,
//   night; overcast grey under the cloud cover c = max(c_min(s), weather).
// - Sun and Moon discs (the Moon lit from the geometric sun: its phase), weighted by 1 − c.
// - Aurorae: curtains on the geographic ring around the seam band (§2.7), at a few kilometres
//   of altitude. The view ray meets the displayed (lowered) shell `y = H − k·d²`, so the ring
//   is seen far away, low on the horizon. Night only (e_vis at the curtain), masked by clouds.
//   The stars are a separate mesh.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};

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
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sky: Sky;

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const HALF_PI: f32 = 1.5707964;
const OBLIQUITY: f32 = 0.40910518;
const DEG: f32 = 0.017453292;

// Visible sun elevation at a point `dx` m east of the camera, at world z (§2.4, §2.5).
fn e_vis_at(dx: f32, z_world: f32) -> f32 {
    let z = z_world - floor(z_world / WORLD) * WORLD;
    let phi = asin(clamp((50000.0 - z) / 50000.0, -1.0, 1.0));
    let s = min(z, WORLD - z);
    let w = smoothstep(0.0, sky.seam.x, s);
    let dec = sky.time.y * w;
    let h = sky.time.x + TAU * dx / WORLD;
    let e = asin(clamp(sin(phi) * sin(dec) + cos(phi) * cos(dec) * cos(h), -1.0, 1.0));
    let r = 1.0 - smoothstep(sky.seam.y, sky.seam.z, s);
    return e - r * (HALF_PI - abs(phi) + OBLIQUITY * w + 4.0 * DEG);
}

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
        let night = 1.0 - smoothstep(-12.0 * DEG, -6.0 * DEG, e_vis_at(rel.x, wz));
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

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = normalize(in.world_position.xyz - view.world_position.xyz);
    let e = sky.sun.w;
    let cloud = sky.sun_geo.w;

    // Base gradient.
    let night_k = smoothstep(-18.0 * DEG, -4.0 * DEG, e);
    let day_k = smoothstep(-2.0 * DEG, 15.0 * DEG, e);
    let zen = mix(mix(vec3<f32>(0.002, 0.003, 0.009), vec3<f32>(0.05, 0.07, 0.2), night_k),
                  vec3<f32>(0.15, 0.32, 0.72), day_k);
    var hor = mix(mix(vec3<f32>(0.006, 0.008, 0.018), vec3<f32>(0.3, 0.25, 0.3), night_k),
                  vec3<f32>(0.62, 0.74, 0.9), day_k);
    // Twilight glow on the sun's side of the horizon.
    let sun_side = max(dot(normalize(d.xz + vec2<f32>(1e-6)), normalize(sky.sun.xz + vec2<f32>(1e-6))), 0.0);
    let twilight = night_k * (1.0 - day_k);
    hor = mix(hor, vec3<f32>(0.95, 0.48, 0.22), twilight * pow(sun_side, 3.0));
    let up = max(d.y, 0.0);
    var col = mix(hor, zen, pow(up, 0.45));
    if d.y < 0.0 {
        col = hor * 0.8;
    }

    // Overcast: grey, lit by the diffuse light of e_vis (§2.7: grey day, black night).
    let sky_light = smoothstep(-6.0 * DEG, 10.0 * DEG, e);
    let grey = vec3<f32>(0.5, 0.52, 0.56) * (0.012 + 0.988 * sky_light);
    col = mix(col, grey, cloud);
    let clear = 1.0 - cloud;

    // Sun disc and halo.
    let cs = dot(d, sky.sun.xyz);
    let disc = smoothstep(cos(0.7 * DEG), cos(0.5 * DEG), cs);
    let halo = pow(max(cs, 0.0), 300.0) * 0.6 + pow(max(cs, 0.0), 12.0) * 0.08 * day_k;
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
        let moon = vec3<f32>(0.85, 0.85, 0.8) * (0.03 + 1.2 * lit);
        col = mix(col, col + moon, edge * clear);
    }

    // Aurorae are lost in a bright sky: only twilight and night skies show them.
    col += aurora(d) * clear * (1.0 - 0.95 * day_k);
    return vec4<f32>(col, 1.0);
}
