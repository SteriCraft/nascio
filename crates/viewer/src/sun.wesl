// Sun at a point of the world (spec §2.4, §2.5), shared by the relief (curvature.wesl) and the
// sky dome (sky.wesl). Its Rust port (`sky::tests::local_sun_port`) is checked against
// `nascio_core::astro` (seam band included), and pinned to this text.

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const HALF_PI: f32 = 1.5707964;
const OBLIQUITY: f32 = 0.40910518;
const SEAM_MARGIN: f32 = 0.06981317; // 4°

struct LocalSun {
    // Towards the visible sun, world axes (x east, y up, z south).
    dir: vec3<f32>,
    e_vis: f32,
    // Seam distance, m.
    s: f32,
}

// Sun `dx` m east of the camera at world z: `hour` is the hour angle at the camera, `dec` the
// declination δ(N), `seam` = (S_FADE, S_DARK, S_LIGHT, -) in m.
//
// Vector form of the spherical triangle: (east, up, north) is a unit vector, so the direction
// needs no azimuth; one `asin` gives e. Only in the seam band, where e_vis is lowered, is the
// direction re-tilted (audit R1: no `atan2` nor second `asin` per pixel elsewhere).
fn local_sun(dx: f32, z_world: f32, hour: f32, dec: f32, seam: vec4<f32>) -> LocalSun {
    let z = z_world - floor(z_world / WORLD) * WORLD;
    let sin_phi = clamp((50000.0 - z) / 50000.0, -1.0, 1.0);
    let cos_phi = sqrt(max(1.0 - sin_phi * sin_phi, 0.0));
    let s = min(z, WORLD - z);
    let w = smoothstep(0.0, seam.x, s);
    let sd = sin(dec * w);
    let cd = cos(dec * w);
    let h = hour + TAU * dx / WORLD;
    let sh = sin(h);
    let ch = cos(h);
    let up = clamp(sin_phi * sd + cos_phi * cd * ch, -1.0, 1.0);
    let east = -cd * sh;
    let north = sd * cos_phi - cd * ch * sin_phi;
    var dir = vec3<f32>(east, up, -north);
    var e_vis = asin(up);
    let r = 1.0 - smoothstep(seam.y, seam.z, s);
    if r > 0.0 {
        e_vis -= r * (HALF_PI - asin(abs(sin_phi)) + OBLIQUITY * w + SEAM_MARGIN);
        let flat = vec2<f32>(east, -north) / max(length(vec2<f32>(east, north)), 1e-6);
        let c = cos(e_vis);
        dir = vec3<f32>(c * flat.x, sin(e_vis), c * flat.y);
    }
    return LocalSun(dir, e_vis, s);
}
