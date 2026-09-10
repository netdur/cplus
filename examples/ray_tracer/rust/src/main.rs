// ===========================================================================
// rt — Shirley "Ray Tracing in One Weekend" tracer, Rust port.
//
// A 1:1 transliteration of examples/ray_tracer/cplus/src/main.cplus, which is
// the reference here. Same scene, same xorshift32 stream, same recursion,
// same by-value Hit and Scattered structs, so the three ports do identical
// arithmetic in identical order.
//
// Rust never contracts `a*b+c` into an FMA, so this build corresponds to the
// other two ports' contraction-off setting and matches their hash for free.
// There is no way to ask rustc for the contracted form, which is why the
// comparison table reports both settings for C and C+ and only one here.
//
// Single-threaded with one shared RNG consumed in a fixed order — that is
// what makes a byte-identical image possible, and also why threading it
// would change the output.
// ===========================================================================

use std::io::Write;

// --- vec3 ------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct V3 { x: f32, y: f32, z: f32 }

#[inline(always)]
fn v(x: f32, y: f32, z: f32) -> V3 { V3 { x, y, z } }
#[inline(always)]
fn add(a: V3, b: V3) -> V3 { v(a.x + b.x, a.y + b.y, a.z + b.z) }
#[inline(always)]
fn sub(a: V3, b: V3) -> V3 { v(a.x - b.x, a.y - b.y, a.z - b.z) }
#[inline(always)]
fn vmul(a: V3, b: V3) -> V3 { v(a.x * b.x, a.y * b.y, a.z * b.z) }
#[inline(always)]
fn scale(a: V3, s: f32) -> V3 { v(a.x * s, a.y * s, a.z * s) }
#[inline(always)]
fn neg(a: V3) -> V3 { v(0.0 - a.x, 0.0 - a.y, 0.0 - a.z) }
#[inline(always)]
fn dot(a: V3, b: V3) -> f32 { a.x * b.x + a.y * b.y + a.z * b.z }
#[inline(always)]
fn len2(a: V3) -> f32 { a.x * a.x + a.y * a.y + a.z * a.z }
#[inline(always)]
fn norm(a: V3) -> V3 { let l = len2(a).sqrt(); scale(a, 1.0 / l) }
#[inline(always)]
fn cross(a: V3, b: V3) -> V3 {
    v(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x)
}
#[inline(always)]
fn reflect(vv: V3, n: V3) -> V3 { let d = dot(vv, n); sub(vv, scale(n, 2.0 * d)) }

// --- xorshift32 ------------------------------------------------------------

#[inline(always)]
fn rng_next(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

#[inline(always)]
fn randf(state: &mut u32) -> f32 {
    (rng_next(state) >> 8) as f32 * (1.0 / 16777216.0)
}

#[inline(always)]
fn rand_in_unit_sphere(state: &mut u32) -> V3 {
    loop {
        // Three draws per attempt, x then y then z.
        let px = 2.0 * randf(state) - 1.0;
        let py = 2.0 * randf(state) - 1.0;
        let pz = 2.0 * randf(state) - 1.0;
        let p = v(px, py, pz);
        if len2(p) < 1.0 { return p; }
    }
}

#[inline(always)]
fn rand_unit_vector(state: &mut u32) -> V3 { norm(rand_in_unit_sphere(state)) }

// --- scene -----------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Sphere {
    center: V3,
    radius: f32,
    mat: i32,        // 0 lambertian, 1 metal, 2 dielectric
    albedo: V3,
    extra: f32,      // metal: fuzz.  dielectric: index of refraction.
}

fn fill_scene() -> [Sphere; 10] {
    let s = |center, radius, mat, albedo, extra| Sphere { center, radius, mat, albedo, extra };
    [
        s(v( 0.0, -1000.0, 0.0), 1000.0, 0, v(0.5, 0.5, 0.5), 0.0),
        s(v( 0.0,    1.0, 0.0),    1.0, 2, v(1.0, 1.0, 1.0), 1.5),
        s(v(-4.0,    1.0, 0.0),    1.0, 0, v(0.4, 0.2, 0.1), 0.0),
        s(v( 4.0,    1.0, 0.0),    1.0, 1, v(0.7, 0.6, 0.5), 0.0),
        s(v(-2.0,    0.3, 1.5),    0.3, 0, v(0.8, 0.3, 0.3), 0.0),
        s(v( 2.0,    0.3, 1.5),    0.3, 1, v(0.8, 0.8, 0.8), 0.3),
        s(v(-1.0,    0.3,-1.5),    0.3, 2, v(1.0, 1.0, 1.0), 1.5),
        s(v( 1.0,    0.3,-1.5),    0.3, 0, v(0.1, 0.2, 0.5), 0.0),
        s(v( 0.0,    0.3, 2.5),    0.3, 1, v(0.6, 0.8, 0.2), 0.0),
        s(v( 0.0,    0.3,-2.5),    0.3, 0, v(0.9, 0.7, 0.2), 0.0),
    ]
}

#[derive(Clone, Copy, Default)]
struct Ray { origin: V3, dir: V3 }

/// By-value Hit with a `valid` flag rather than an out-pointer, matching the
/// reference: SROA splits it into registers.
#[derive(Clone, Copy, Default)]
struct Hit {
    valid: bool,
    t: f32,
    p: V3,
    normal: V3,
    front_face: i32,
    sphere_idx: i32,
}

#[inline(always)]
fn miss() -> Hit { Hit::default() }

fn sphere_hit(scene: &[Sphere; 10], idx: i32, r: Ray, t_min: f32, t_max: f32) -> Hit {
    let s = scene[idx as usize];
    let oc = sub(r.origin, s.center);
    // The ray direction is NOT normalized here, so `a` is carried through.
    let a = len2(r.dir);
    let half_b = dot(oc, r.dir);
    let c = len2(oc) - s.radius * s.radius;
    let disc = half_b * half_b - a * c;
    if disc < 0.0 { return miss(); }
    let sd = disc.sqrt();
    let mut root = (0.0 - half_b - sd) / a;
    if root <= t_min || root >= t_max {
        root = (0.0 - half_b + sd) / a;
        if root <= t_min || root >= t_max { return miss(); }
    }
    let p = add(r.origin, scale(r.dir, root));
    let outward = scale(sub(p, s.center), 1.0 / s.radius);
    let ff: i32 = if dot(r.dir, outward) < 0.0 { 1 } else { 0 };
    let mut n = outward;
    if ff == 0 { n = neg(outward); }
    Hit { valid: true, t: root, p, normal: n, front_face: ff, sphere_idx: idx }
}

fn world_hit(scene: &[Sphere; 10], n_spheres: i32, r: Ray, t_min: f32, t_max: f32) -> Hit {
    let mut best = miss();
    let mut closest = t_max;
    for i in 0..n_spheres {
        let h = sphere_hit(scene, i, r, t_min, closest);
        if h.valid {
            closest = h.t;
            best = h;
        }
    }
    best
}

#[inline(always)]
fn schlick(cosv: f32, ref_idx: f32) -> f32 {
    let mut r0 = (1.0 - ref_idx) / (1.0 + ref_idx);
    r0 = r0 * r0;
    let om = 1.0 - cosv;
    r0 + (1.0 - r0) * om * om * om * om * om
}

#[derive(Clone, Copy, Default)]
struct Scattered { valid: bool, atten: V3, scattered: Ray }

#[inline(always)]
fn absorbed() -> Scattered { Scattered::default() }

fn scatter(scene: &[Sphere; 10], state: &mut u32, r_in: Ray, rec: Hit) -> Scattered {
    let s = scene[rec.sphere_idx as usize];
    if s.mat == 0 {
        let mut dir = add(rec.normal, rand_unit_vector(state));
        if dir.x.abs() < 1e-8 {
            if dir.y.abs() < 1e-8 {
                if dir.z.abs() < 1e-8 {
                    dir = rec.normal;
                }
            }
        }
        return Scattered { valid: true, atten: s.albedo, scattered: Ray { origin: rec.p, dir } };
    }
    if s.mat == 1 {
        let refl = reflect(norm(r_in.dir), rec.normal);
        let dir = add(refl, scale(rand_in_unit_sphere(state), s.extra));
        let d = dot(dir, rec.normal);
        if d <= 0.0 { return absorbed(); }
        return Scattered { valid: true, atten: s.albedo, scattered: Ray { origin: rec.p, dir } };
    }
    // Dielectric.
    let ratio = if rec.front_face == 1 { 1.0 / s.extra } else { s.extra };
    let unit_dir = norm(r_in.dir);
    let mut cos_theta = 0.0 - dot(unit_dir, rec.normal);
    if cos_theta > 1.0 { cos_theta = 1.0; }
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    let cannot = (ratio * sin_theta) > 1.0;
    let dir;
    if cannot || schlick(cos_theta, ratio) > randf(state) {
        dir = reflect(unit_dir, rec.normal);
    } else {
        let r_perp = scale(add(unit_dir, scale(rec.normal, cos_theta)), ratio);
        let mut k = 1.0 - len2(r_perp);
        if k < 0.0 { k = 0.0; }
        let r_para = scale(rec.normal, 0.0 - k.sqrt());
        dir = add(r_perp, r_para);
    }
    Scattered { valid: true, atten: v(1.0, 1.0, 1.0), scattered: Ray { origin: rec.p, dir } }
}

/// Recursive, matching the reference.
fn ray_color(scene: &[Sphere; 10], n_spheres: i32, state: &mut u32, r: Ray, depth: i32) -> V3 {
    if depth <= 0 { return v(0.0, 0.0, 0.0); }
    let rec = world_hit(scene, n_spheres, r, 0.001, 1e30);
    if rec.valid {
        let sc = scatter(scene, state, r, rec);
        if sc.valid {
            let c = ray_color(scene, n_spheres, state, sc.scattered, depth - 1);
            return vmul(sc.atten, c);
        }
        return v(0.0, 0.0, 0.0);
    }
    let ud = norm(r.dir);
    let t = 0.5 * (ud.y + 1.0);
    add(scale(v(1.0, 1.0, 1.0), 1.0 - t), scale(v(0.5, 0.7, 1.0), t))
}

// --- camera ----------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Camera { origin: V3, ll: V3, hor: V3, ver: V3 }

fn cam_init(look_from: V3, look_at: V3, vup: V3, vfov_deg: f32, aspect: f32) -> Camera {
    let theta = vfov_deg * 3.14159265358979323846_f32 / 180.0;
    let h = (theta * 0.5).tan();
    let vh = 2.0 * h;
    let vw = aspect * vh;
    let w = norm(sub(look_from, look_at));
    let u = norm(cross(vup, w));
    let vv = cross(w, u);
    Camera {
        origin: look_from,
        hor: scale(u, vw),
        ver: scale(vv, vh),
        ll: sub(sub(sub(look_from, scale(scale(u, vw), 0.5)), scale(scale(vv, vh), 0.5)), w),
    }
}

#[inline(always)]
fn cam_ray(c: &Camera, s: f32, t: f32) -> Ray {
    let origin = c.origin;
    let (ll, hor, ver) = (c.ll, c.hor, c.ver);
    Ray { origin, dir: sub(add(add(ll, scale(hor, s)), scale(ver, t)), origin) }
}

#[inline(always)]
fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo { return lo; }
    if x > hi { return hi; }
    x
}

// --- main ------------------------------------------------------------------

fn main() {
    let (width, height, samples, max_depth) = (800i32, 450i32, 32i32, 15i32);
    let n_spheres = 10i32;

    let scene = fill_scene();
    let mut rng: u32 = 0x1234_5678;

    let aspect = width as f32 / height as f32;
    let cam = cam_init(v(13.0, 2.0, 3.0), v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), 20.0, aspect);

    let pixel_bytes = width as usize * height as usize * 3;
    let mut pixels = vec![0u8; pixel_bytes];

    let inv_samples = 1.0 / samples as f32;

    let mut j = height - 1;
    while j >= 0 {
        for i in 0..width {
            let mut col = v(0.0, 0.0, 0.0);
            for _ in 0..samples {
                let u = (i as f32 + randf(&mut rng)) / (width - 1) as f32;
                let vv = (j as f32 + randf(&mut rng)) / (height - 1) as f32;
                col = add(col, ray_color(&scene, n_spheres, &mut rng,
                                         cam_ray(&cam, u, vv), max_depth));
            }
            let r = clampf((col.x * inv_samples).sqrt(), 0.0, 0.999);
            let g = clampf((col.y * inv_samples).sqrt(), 0.0, 0.999);
            let b = clampf((col.z * inv_samples).sqrt(), 0.0, 0.999);
            let row_off = (height - 1 - j) as usize * width as usize * 3;
            let px_off = row_off + i as usize * 3;
            pixels[px_off] = (256.0 * r) as u8;
            pixels[px_off + 1] = (256.0 * g) as u8;
            pixels[px_off + 2] = (256.0 * b) as u8;
        }
        j -= 1;
    }

    let f = std::fs::File::create("out.ppm").expect("out.ppm");
    let mut f = std::io::BufWriter::new(f);
    f.write_all(b"P6\n800 450\n255\n").unwrap();
    f.write_all(&pixels).unwrap();
    f.flush().unwrap();
}
