/* ===========================================================================
 * rt — Shirley "Ray Tracing in One Weekend" tracer, C reference.
 *
 * This is a 1:1 transliteration of examples/ray_tracer/cplus/src/main.cplus,
 * which is the reference implementation here rather than the other way round.
 * Same scene, same xorshift32 stream, same recursion, same by-value Hit and
 * Scattered structs — the point of the comparison is that the three ports do
 * identical arithmetic in identical order, so any difference in the numbers
 * is the language and its toolchain.
 *
 * Single-threaded and no acceleration structure, deliberately: 10 spheres
 * tested linearly. A BVH would make the tracer faster and the comparison
 * worse, because most of the runtime would move into one hand-tuned data
 * structure rather than into the language's ordinary code.
 *
 * Determinism note: one shared RNG state consumed in a fixed sequential
 * order. That is what makes a byte-identical image possible at all, and it
 * is also why this cannot be threaded without changing the output.
 * ===========================================================================
 */

#include <fcntl.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

/* --- vec3 -------------------------------------------------------------- */

typedef struct { float x, y, z; } V3;

static inline V3 V(float x, float y, float z) { V3 r = {x, y, z}; return r; }
static inline V3 add(V3 a, V3 b)   { return V(a.x + b.x, a.y + b.y, a.z + b.z); }
static inline V3 sub(V3 a, V3 b)   { return V(a.x - b.x, a.y - b.y, a.z - b.z); }
static inline V3 vmul(V3 a, V3 b)  { return V(a.x * b.x, a.y * b.y, a.z * b.z); }
static inline V3 scale(V3 a, float s) { return V(a.x * s, a.y * s, a.z * s); }
static inline V3 neg(V3 a)         { return V(0.0f - a.x, 0.0f - a.y, 0.0f - a.z); }
static inline float dot(V3 a, V3 b)  { return a.x * b.x + a.y * b.y + a.z * b.z; }
static inline float len2(V3 a)       { return a.x*a.x + a.y*a.y + a.z*a.z; }
static inline V3 norm(V3 a) { float l = sqrtf(len2(a)); return scale(a, 1.0f / l); }
static inline V3 cross(V3 a, V3 b) {
    return V(a.y*b.z - a.z*b.y, a.z*b.x - a.x*b.z, a.x*b.y - a.y*b.x);
}
static inline V3 reflect(V3 v, V3 n) {
    float d = dot(v, n);
    return sub(v, scale(n, 2.0f * d));
}

/* --- xorshift32 -------------------------------------------------------- */

static inline uint32_t rng_next(uint32_t *restrict state) {
    uint32_t x = state[0];
    x = x ^ (x << 13);
    x = x ^ (x >> 17);
    x = x ^ (x << 5);
    state[0] = x;
    return x;
}

static inline float randf(uint32_t *restrict state) {
    return (float)(rng_next(state) >> 8) * (1.0f / 16777216.0f);
}

static inline V3 rand_in_unit_sphere(uint32_t *restrict state) {
    V3 p = V(0.0f, 0.0f, 0.0f);
    int done = 0;
    while (!done) {
        /* Three draws per attempt, x then y then z. */
        float px = 2.0f * randf(state) - 1.0f;
        float py = 2.0f * randf(state) - 1.0f;
        float pz = 2.0f * randf(state) - 1.0f;
        p = V(px, py, pz);
        if (len2(p) < 1.0f) done = 1;
    }
    return p;
}

static inline V3 rand_unit_vector(uint32_t *restrict state) {
    return norm(rand_in_unit_sphere(state));
}

/* --- scene ------------------------------------------------------------- */

typedef struct {
    V3 center;
    float radius;
    int32_t mat;      /* 0 lambertian, 1 metal, 2 dielectric */
    V3 albedo;
    float extra;      /* metal: fuzz.  dielectric: index of refraction. */
} Sphere;

static void fill_scene(Sphere *restrict s) {
    s[0] = (Sphere){V( 0.0f,-1000.0f, 0.0f), 1000.0f, 0, V(0.5f,0.5f,0.5f), 0.0f};
    s[1] = (Sphere){V( 0.0f,   1.0f, 0.0f),    1.0f, 2, V(1.0f,1.0f,1.0f), 1.5f};
    s[2] = (Sphere){V(-4.0f,   1.0f, 0.0f),    1.0f, 0, V(0.4f,0.2f,0.1f), 0.0f};
    s[3] = (Sphere){V( 4.0f,   1.0f, 0.0f),    1.0f, 1, V(0.7f,0.6f,0.5f), 0.0f};
    s[4] = (Sphere){V(-2.0f,   0.3f, 1.5f),    0.3f, 0, V(0.8f,0.3f,0.3f), 0.0f};
    s[5] = (Sphere){V( 2.0f,   0.3f, 1.5f),    0.3f, 1, V(0.8f,0.8f,0.8f), 0.3f};
    s[6] = (Sphere){V(-1.0f,   0.3f,-1.5f),    0.3f, 2, V(1.0f,1.0f,1.0f), 1.5f};
    s[7] = (Sphere){V( 1.0f,   0.3f,-1.5f),    0.3f, 0, V(0.1f,0.2f,0.5f), 0.0f};
    s[8] = (Sphere){V( 0.0f,   0.3f, 2.5f),    0.3f, 1, V(0.6f,0.8f,0.2f), 0.0f};
    s[9] = (Sphere){V( 0.0f,   0.3f,-2.5f),    0.3f, 0, V(0.9f,0.7f,0.2f), 0.0f};
}

typedef struct { V3 origin, dir; } Ray;

/* By-value Hit with a `valid` flag rather than an out-pointer: SROA splits
 * it into registers, and it keeps the three ports structurally identical. */
typedef struct {
    int valid;
    float t;
    V3 p;
    V3 normal;
    int32_t front_face;
    int32_t sphere_idx;
} Hit;

static inline Hit miss(void) {
    Hit h = {0, 0.0f, {0.0f,0.0f,0.0f}, {0.0f,0.0f,0.0f}, 0, 0};
    return h;
}

static Hit sphere_hit(const Sphere *restrict scene, int32_t idx, Ray r,
                      float t_min, float t_max) {
    Sphere s = scene[idx];
    V3 oc = sub(r.origin, s.center);
    /* The ray direction is NOT normalized here, so `a` is carried through. */
    float a = len2(r.dir);
    float half_b = dot(oc, r.dir);
    float c = len2(oc) - s.radius * s.radius;
    float disc = half_b * half_b - a * c;
    if (disc < 0.0f) return miss();
    float sd = sqrtf(disc);
    float root = (0.0f - half_b - sd) / a;
    if (root <= t_min || root >= t_max) {
        root = (0.0f - half_b + sd) / a;
        if (root <= t_min || root >= t_max) return miss();
    }
    V3 p = add(r.origin, scale(r.dir, root));
    V3 outward = scale(sub(p, s.center), 1.0f / s.radius);
    int32_t ff = dot(r.dir, outward) < 0.0f ? 1 : 0;
    V3 n = outward;
    if (ff == 0) n = neg(outward);
    Hit h = {1, root, p, n, ff, idx};
    return h;
}

static Hit world_hit(const Sphere *restrict scene, int32_t n_spheres, Ray r,
                     float t_min, float t_max) {
    Hit best = miss();
    float closest = t_max;
    for (int32_t i = 0; i < n_spheres; i++) {
        Hit h = sphere_hit(scene, i, r, t_min, closest);
        if (h.valid) {
            closest = h.t;
            best = h;
        }
    }
    return best;
}

static inline float schlick(float cosv, float ref_idx) {
    float r0 = (1.0f - ref_idx) / (1.0f + ref_idx);
    r0 = r0 * r0;
    float om = 1.0f - cosv;
    return r0 + (1.0f - r0) * om*om*om*om*om;
}

typedef struct { int valid; V3 atten; Ray scattered; } Scattered;

static inline Scattered absorbed(void) {
    Scattered s = {0, {0.0f,0.0f,0.0f}, {{0.0f,0.0f,0.0f},{0.0f,0.0f,0.0f}}};
    return s;
}

static Scattered scatter(const Sphere *restrict scene, uint32_t *restrict state,
                         Ray r_in, Hit rec) {
    Sphere s = scene[rec.sphere_idx];
    if (s.mat == 0) {
        V3 dir = add(rec.normal, rand_unit_vector(state));
        if (fabsf(dir.x) < 1e-8f) {
            if (fabsf(dir.y) < 1e-8f) {
                if (fabsf(dir.z) < 1e-8f) {
                    dir = rec.normal;
                }
            }
        }
        Scattered out = {1, s.albedo, {rec.p, dir}};
        return out;
    }
    if (s.mat == 1) {
        V3 refl = reflect(norm(r_in.dir), rec.normal);
        V3 dir = add(refl, scale(rand_in_unit_sphere(state), s.extra));
        float d = dot(dir, rec.normal);
        if (d <= 0.0f) return absorbed();
        Scattered out = {1, s.albedo, {rec.p, dir}};
        return out;
    }
    /* Dielectric. */
    float ratio = rec.front_face == 1 ? (1.0f / s.extra) : s.extra;
    V3 unit_dir = norm(r_in.dir);
    float cos_theta = 0.0f - dot(unit_dir, rec.normal);
    if (cos_theta > 1.0f) cos_theta = 1.0f;
    float sin_theta = sqrtf(1.0f - cos_theta*cos_theta);
    int cannot = (ratio * sin_theta) > 1.0f;
    V3 dir = V(0.0f, 0.0f, 0.0f);
    if (cannot || schlick(cos_theta, ratio) > randf(state)) {
        dir = reflect(unit_dir, rec.normal);
    } else {
        V3 r_perp = scale(add(unit_dir, scale(rec.normal, cos_theta)), ratio);
        float k = 1.0f - len2(r_perp);
        if (k < 0.0f) k = 0.0f;
        V3 r_para = scale(rec.normal, 0.0f - sqrtf(k));
        dir = add(r_perp, r_para);
    }
    Scattered out = {1, V(1.0f,1.0f,1.0f), {rec.p, dir}};
    return out;
}

/* Recursive, matching the reference. */
static V3 ray_color(const Sphere *restrict scene, int32_t n_spheres,
                    uint32_t *restrict state, Ray r, int32_t depth) {
    if (depth <= 0) return V(0.0f, 0.0f, 0.0f);
    Hit rec = world_hit(scene, n_spheres, r, 0.001f, 1e30f);
    if (rec.valid) {
        Scattered sc = scatter(scene, state, r, rec);
        if (sc.valid) {
            V3 c = ray_color(scene, n_spheres, state, sc.scattered, depth - 1);
            return vmul(sc.atten, c);
        }
        return V(0.0f, 0.0f, 0.0f);
    }
    V3 ud = norm(r.dir);
    float t = 0.5f * (ud.y + 1.0f);
    return add(scale(V(1.0f,1.0f,1.0f), 1.0f - t), scale(V(0.5f,0.7f,1.0f), t));
}

/* --- camera ------------------------------------------------------------ */

typedef struct { V3 origin, ll, hor, ver; } Camera;

static void cam_init(Camera *restrict c, V3 look_from, V3 look_at, V3 vup,
                     float vfov_deg, float aspect) {
    float theta = vfov_deg * 3.14159265358979323846f / 180.0f;
    float h = tanf(theta * 0.5f);
    float vh = 2.0f * h;
    float vw = aspect * vh;
    V3 w = norm(sub(look_from, look_at));
    V3 u = norm(cross(vup, w));
    V3 v = cross(w, u);
    c[0].origin = look_from;
    c[0].hor = scale(u, vw);
    c[0].ver = scale(v, vh);
    c[0].ll = sub(sub(sub(look_from, scale(scale(u, vw), 0.5f)),
                      scale(scale(v, vh), 0.5f)), w);
}

static inline Ray cam_ray(const Camera *restrict c, float s, float t) {
    V3 origin = c[0].origin;
    V3 ll = c[0].ll, hor = c[0].hor, ver = c[0].ver;
    Ray r = {origin, sub(add(add(ll, scale(hor, s)), scale(ver, t)), origin)};
    return r;
}

static inline float clampf(float x, float lo, float hi) {
    if (x < lo) return lo;
    if (x > hi) return hi;
    return x;
}

/* --- main -------------------------------------------------------------- */

int main(void) {
    const int32_t width = 800, height = 450, samples = 32, max_depth = 15;
    const int32_t n_spheres = 10;

    Sphere *scene = (Sphere *)malloc((size_t)n_spheres * sizeof(Sphere));
    fill_scene(scene);

    uint32_t *rng = (uint32_t *)malloc(4);
    rng[0] = 0x12345678u;

    float aspect = (float)width / (float)height;

    Camera *cam = (Camera *)malloc(sizeof(Camera));
    cam_init(cam, V(13.0f, 2.0f, 3.0f), V(0.0f, 0.0f, 0.0f), V(0.0f, 1.0f, 0.0f),
             20.0f, aspect);

    size_t pixel_bytes = (size_t)width * (size_t)height * 3u;
    uint8_t *pixels = (uint8_t *)malloc(pixel_bytes);

    float inv_samples = 1.0f / (float)samples;

    for (int32_t j = height - 1; j >= 0; j--) {
        for (int32_t i = 0; i < width; i++) {
            V3 col = V(0.0f, 0.0f, 0.0f);
            for (int32_t s = 0; s < samples; s++) {
                float u = ((float)i + randf(rng)) / (float)(width - 1);
                float v = ((float)j + randf(rng)) / (float)(height - 1);
                col = add(col, ray_color(scene, n_spheres, rng,
                                         cam_ray(cam, u, v), max_depth));
            }
            float r = clampf(sqrtf(col.x * inv_samples), 0.0f, 0.999f);
            float g = clampf(sqrtf(col.y * inv_samples), 0.0f, 0.999f);
            float b = clampf(sqrtf(col.z * inv_samples), 0.0f, 0.999f);
            size_t row_off = (size_t)(height - 1 - j) * (size_t)width * 3u;
            size_t px_off = row_off + (size_t)i * 3u;
            pixels[px_off + 0] = (uint8_t)(256.0f * r);
            pixels[px_off + 1] = (uint8_t)(256.0f * g);
            pixels[px_off + 2] = (uint8_t)(256.0f * b);
        }
    }

    int flags = O_WRONLY | O_CREAT | O_TRUNC;
    int fd = open("out.ppm", flags, 0644);
    if (fd < 0) {
        free(pixels); free(scene); free(rng); free(cam);
        return 2;
    }
    const char *header = "P6\n800 450\n255\n";
    ssize_t hw = write(fd, header, 15);
    (void)hw;

    size_t written = 0;
    while (written < pixel_bytes) {
        size_t remaining = pixel_bytes - written;
        ssize_t w = write(fd, pixels + written, remaining);
        if (w <= 0) break;
        written += (size_t)w;
    }
    close(fd);

    free(pixels);
    free(scene);
    free(rng);
    free(cam);
    return 0;
}
