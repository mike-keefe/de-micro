mod net;

use macroquad::audio::{load_sound_from_bytes, play_sound, PlaySoundParams, Sound};
use macroquad::camera::Camera;
use macroquad::models::{draw_mesh, Mesh, Vertex};
use macroquad::prelude::*;
use macroquad::rand::gen_range;
use net::{BotNet, ClientEvent, Event, HitClaim, HostEvent, PlayerInput, PlayerNet, ShotMsg, Snapshot, C2S, S2C};

const GRAVITY: f32 = 20.0;
const PLAYER_SPEED: f32 = 6.3;
const WALK_SPEED: f32 = 2.9;
const JUMP_VEL: f32 = 7.2;
const EYE: f32 = 1.6;
const PLAYER_R: f32 = 0.35;
const PLAYER_H: f32 = 1.8;
const BOT_SPEED: f32 = 5.0;
const ROUND_TIME: f32 = 105.0;
const BOMB_TIME: f32 = 40.0;
const PLANT_TIME: f32 = 3.5;
const DEFUSE_TIME: f32 = 5.0;
const BOT_DEFUSE_TIME: f32 = 8.0;
const FREEZE_TIME: f32 = 2.5;
const WIN_SCORE: i32 = 8;
const NET_RATE: f32 = 1.0 / 30.0;

const BOT_NAMES: [&str; 10] = [
    "Phoenix", "Viper", "Sandman", "Crusher", "Steel", "Hawk", "Reaper", "Falcon", "Brute", "Ghost",
];
const SPAWN_X: [f32; 6] = [-10.0, -6.0, -2.0, 2.0, 6.0, 10.0];

#[derive(Clone, Copy, PartialEq)]
enum Team {
    Ct,
    T,
}

impl Team {
    fn is_t(self) -> bool {
        self == Team::T
    }
    fn from_t(t: bool) -> Team {
        if t {
            Team::T
        } else {
            Team::Ct
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Menu,
    Freeze,
    Live,
    Post,
}

#[derive(Clone, Copy, PartialEq)]
enum Ent {
    Me,
    Remote(u8),
    Bot(usize),
}

// ---------- math / collision ----------

#[derive(Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

fn ray_aabb(o: Vec3, d: Vec3, b: &Aabb) -> Option<f32> {
    let inv = vec3(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
    let t1 = (b.min - o) * inv;
    let t2 = (b.max - o) * inv;
    let tmin = t1.min(t2);
    let tmax = t1.max(t2);
    let enter = tmin.x.max(tmin.y).max(tmin.z);
    let exit = tmax.x.min(tmax.y).min(tmax.z);
    if exit >= enter.max(0.0) {
        Some(enter.max(0.0))
    } else {
        None
    }
}

fn nearest_wall_hit(o: Vec3, d: Vec3, max_dist: f32, walls: &[Aabb]) -> f32 {
    let mut best = max_dist;
    for w in walls {
        if let Some(t) = ray_aabb(o, d, w) {
            if t < best {
                best = t;
            }
        }
    }
    best
}

fn los_clear(a: Vec3, b: Vec3, walls: &[Aabb]) -> bool {
    let d = b - a;
    let dist = d.length();
    if dist < 0.01 {
        return true;
    }
    let dir = d / dist;
    nearest_wall_hit(a, dir, dist, walls) >= dist - 0.01
}

fn body_overlaps(pos: Vec3, r: f32, h: f32, w: &Aabb) -> bool {
    pos.x + r > w.min.x
        && pos.x - r < w.max.x
        && pos.y + h > w.min.y
        && pos.y < w.max.y
        && pos.z + r > w.min.z
        && pos.z - r < w.max.z
}

fn move_collide(pos: &mut Vec3, vy: &mut f32, r: f32, h: f32, delta: Vec3, walls: &[Aabb]) -> bool {
    pos.x += delta.x;
    for w in walls {
        if body_overlaps(*pos, r, h, w) {
            pos.x = if delta.x > 0.0 { w.min.x - r } else { w.max.x + r };
        }
    }
    pos.z += delta.z;
    for w in walls {
        if body_overlaps(*pos, r, h, w) {
            pos.z = if delta.z > 0.0 { w.min.z - r } else { w.max.z + r };
        }
    }
    pos.y += delta.y;
    let mut on_ground = false;
    if pos.y <= 0.0 {
        pos.y = 0.0;
        *vy = 0.0;
        on_ground = true;
    }
    for w in walls {
        if body_overlaps(*pos, r, h, w) {
            if delta.y <= 0.0 && pos.y > w.max.y - 1.1 {
                pos.y = w.max.y;
                *vy = 0.0;
                on_ground = true;
            } else if delta.y > 0.0 {
                pos.y = w.min.y - h;
                *vy = 0.0;
            }
        }
    }
    on_ground
}

fn shade(c: Color, k: f32) -> Color {
    Color::new(c.r * k, c.g * k, c.b * k, c.a)
}

// ---------- baked lighting ----------
// A small directional + hemispheric model baked into vertex colors. Gives the
// flat-shaded geometry real form: a warm key "sun", cool sky fill, and ambient
// occlusion in the lower portions of objects so nothing reads as a flat slab.

fn sun_dir() -> Vec3 {
    vec3(-0.32, 0.86, 0.40).normalize()
}

fn apply_light(base: Color, n: Vec3, ao: f32) -> Color {
    let s = sun_dir();
    let ndl = n.dot(s).max(0.0);
    // sky term: surfaces facing up catch more of the bright sky dome
    let sky = (n.y * 0.5 + 0.5).clamp(0.0, 1.0);
    let amb = 0.30 + 0.14 * sky;
    let sun_c = vec3(1.22, 1.10, 0.88);
    let sky_c = vec3(0.46, 0.55, 0.72);
    let lr = sun_c.x * ndl * 0.82 + sky_c.x * amb;
    let lg = sun_c.y * ndl * 0.82 + sky_c.y * amb;
    let lb = sun_c.z * ndl * 0.82 + sky_c.z * amb;
    Color::new(
        (base.r * lr * ao).clamp(0.0, 1.0),
        (base.g * lg * ao).clamp(0.0, 1.0),
        (base.b * lb * ao).clamp(0.0, 1.0),
        base.a,
    )
}

// cheap value noise for breaking up flat surfaces
fn vnoise(p: Vec3) -> f32 {
    let mut h = (p.x * 12.989 + p.y * 78.233 + p.z * 37.719).sin() * 43758.547;
    h -= h.floor();
    h
}

fn angle_lerp(a: f32, b: f32, t: f32) -> f32 {
    let mut d = b - a;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    a + d * t
}

fn v3(a: [f32; 3]) -> Vec3 {
    vec3(a[0], a[1], a[2])
}

fn a3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

// ---------- audio synth ----------

const SR: u32 = 22050;

struct SynthRng(u32);
impl SynthRng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / 8388608.0 - 1.0
    }
}

fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&SR.to_le_bytes());
    v.extend_from_slice(&(SR * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    v
}

fn gen_shot(dur: f32, lp: f32, vol: f32, seed: u32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut rng = SynthRng(seed);
    let mut out = vec![0.0; n];
    let mut y = 0.0;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let env = (1.0 - t).powf(2.6);
        let x = rng.next();
        y += lp * (x - y);
        out[i] = (y * 1.4 + x * 0.25) * env * vol;
    }
    out
}

// Layered weapon report: a bright crack transient, a punchy filtered-noise body,
// and a low rumble tail. Reads far more like a real firearm than flat noise.
fn gen_gun(dur: f32, body_lp: f32, punch: f32, vol: f32, seed: u32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut rng = SynthRng(seed);
    let mut out = vec![0.0; n];
    let mut lp = 0.0f32;
    let mut tail = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let x = rng.next();
        lp += body_lp * (x - lp);
        let body_env = (1.0 - t).powf(2.0);
        let crack_env = (-(t * 60.0)).exp();
        tail += 0.02 * (x - tail);
        let tail_env = (1.0 - t).powf(1.1);
        out[i] = ((lp * 1.5 + x * 0.28) * body_env * punch
            + x * crack_env * 0.55
            + tail * 2.2 * tail_env * 0.3)
            * vol;
    }
    out
}

// short filtered-noise whoosh for knife swings / nade throws
fn gen_whoosh(dur: f32, vol: f32, seed: u32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut rng = SynthRng(seed);
    let mut out = vec![0.0; n];
    let mut bp = 0.0f32;
    let mut lp = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let x = rng.next();
        let cut = 0.06 + 0.25 * (1.0 - (t - 0.5).abs() * 2.0).max(0.0);
        lp += cut * (x - lp);
        bp = lp - bp * 0.5;
        let env = (1.0 - (t - 0.4).abs() * 2.0).max(0.0).powf(1.3);
        out[i] = bp * env * vol;
    }
    out
}

// looping ambient wind bed: slow-modulated low noise, ends faded for clean loop
fn gen_wind(dur: f32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut rng = SynthRng(8123);
    let mut out = vec![0.0; n];
    let mut lp = 0.0f32;
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let x = rng.next();
        lp += 0.01 * (x - lp);
        let lfo = 0.55 + 0.45 * (t * 0.4 * std::f32::consts::TAU).sin();
        let lfo2 = 0.6 + 0.4 * (t * 0.13 * std::f32::consts::TAU).sin();
        // fade the first/last 10% so the loop seam is inaudible
        let p = i as f32 / n as f32;
        let edge = (p / 0.1).min(1.0).min((1.0 - p) / 0.1);
        out[i] = lp * 3.0 * lfo * lfo2 * 0.16 * edge;
    }
    out
}

fn gen_ui(freq: f32, dur: f32, vol: f32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut out = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let env = (-(i as f32 / n as f32) * 7.0).exp();
        let s = (t * freq * std::f32::consts::TAU).sin();
        out[i] = (s * 0.7 + (t * freq * 2.0 * std::f32::consts::TAU).sin() * 0.3) * env * vol;
    }
    out
}

fn gen_beep(freq: f32, dur: f32, vol: f32) -> Vec<f32> {
    let n = (SR as f32 * dur) as usize;
    let mut out = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let env = (1.0 - i as f32 / n as f32).powf(0.6);
        out[i] = (t * freq * std::f32::consts::TAU).sin() * env * vol;
    }
    out
}

fn gen_explosion() -> Vec<f32> {
    let n = (SR as f32 * 1.3) as usize;
    let mut rng = SynthRng(777);
    let mut out = vec![0.0; n];
    let mut y = 0.0;
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let prog = i as f32 / n as f32;
        let env = (1.0 - prog).powf(1.8);
        let x = rng.next();
        y += 0.035 * (x - y);
        let rumble = (t * 52.0 * std::f32::consts::TAU).sin() * 0.5;
        out[i] = (y * 2.2 + rumble) * env;
    }
    out
}

fn concat(mut a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    a.extend(b);
    a
}

struct Sounds {
    ak: Sound,
    m4: Sound,
    mp5: Sound,
    deagle: Sound,
    pistol: Sound,
    awp: Sound,
    knife: Sound,
    far: Sound,
    hit: Sound,
    headshot: Sound,
    reload: Sound,
    draw: Sound,
    dryfire: Sound,
    beep: Sound,
    planted: Sound,
    defused: Sound,
    explosion: Sound,
    death: Sound,
    step: Sound,
    step2: Sound,
    ambient: Sound,
    ui_click: Sound,
    ui_hover: Sound,
    flash: Sound,
    pickup: Sound,
    whistle: Sound,
}

impl Sounds {
    async fn load() -> Sounds {
        async fn s(samples: Vec<f32>) -> Sound {
            load_sound_from_bytes(&wav_bytes(&samples)).await.unwrap()
        }
        Sounds {
            ak: s(gen_gun(0.22, 0.42, 1.0, 0.95, 1234)).await,
            m4: s(gen_gun(0.17, 0.52, 0.9, 0.85, 6611)).await,
            mp5: s(gen_gun(0.12, 0.58, 0.78, 0.72, 4242)).await,
            deagle: s(gen_gun(0.32, 0.30, 1.25, 1.0, 909)).await,
            pistol: s(gen_gun(0.12, 0.55, 0.72, 0.72, 4321)).await,
            awp: s(gen_gun(0.55, 0.22, 1.35, 1.0, 5577)).await,
            knife: s(gen_whoosh(0.18, 0.6, 717)).await,
            far: s(gen_shot(0.2, 0.06, 0.55, 9999)).await,
            hit: s(gen_ui(1700.0, 0.05, 0.4)).await,
            headshot: s(gen_ui(2300.0, 0.07, 0.5)).await,
            reload: s(gen_shot(0.05, 0.85, 0.45, 55)).await,
            draw: s(gen_whoosh(0.1, 0.3, 313)).await,
            dryfire: s(gen_ui(220.0, 0.05, 0.3)).await,
            beep: s(gen_beep(980.0, 0.09, 0.5)).await,
            planted: s(concat(gen_beep(1318.0, 0.12, 0.5), gen_beep(1568.0, 0.18, 0.5))).await,
            defused: s(concat(gen_beep(880.0, 0.15, 0.5), gen_beep(1318.0, 0.25, 0.5))).await,
            explosion: s(gen_explosion()).await,
            death: s(gen_shot(0.3, 0.05, 0.7, 31337)).await,
            step: s(gen_shot(0.045, 0.16, 0.5, 222)).await,
            step2: s(gen_shot(0.05, 0.13, 0.5, 9182)).await,
            ambient: s(gen_wind(4.0)).await,
            ui_click: s(gen_ui(660.0, 0.06, 0.35)).await,
            ui_hover: s(gen_ui(440.0, 0.03, 0.18)).await,
            flash: s(concat(gen_gun(0.1, 0.7, 0.6, 0.6, 31), gen_beep(4200.0, 0.6, 0.25))).await,
            pickup: s(gen_ui(880.0, 0.08, 0.3)).await,
            whistle: s(concat(gen_beep(1568.0, 0.14, 0.4), gen_beep(2093.0, 0.2, 0.4))).await,
        }
    }
}

fn play_looped(s: &Sound, vol: f32) {
    play_sound(s, PlaySoundParams { looped: true, volume: vol.clamp(0.0, 1.0) });
}

fn play(s: &Sound, vol: f32) {
    if vol < 0.02 {
        return;
    }
    play_sound(
        s,
        PlaySoundParams {
            looped: false,
            volume: vol.clamp(0.0, 1.0),
        },
    );
}

fn dist_vol(from: Vec3, to: Vec3, base: f32) -> f32 {
    let d = from.distance(to);
    base * (1.0 - d / 75.0).clamp(0.0, 1.0)
}

// ---------- map ----------

fn push_quad(verts: &mut Vec<Vertex>, inds: &mut Vec<u16>, p: [Vec3; 4], c: Color) {
    let base = verts.len() as u16;
    for q in p {
        verts.push(Vertex::new(q.x, q.y, q.z, 0.0, 0.0, c));
    }
    inds.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

// quad with explicit per-vertex colours (used for AO gradients)
fn push_quad_c(verts: &mut Vec<Vertex>, inds: &mut Vec<u16>, p: [Vec3; 4], c: [Color; 4]) {
    let base = verts.len() as u16;
    for i in 0..4 {
        verts.push(Vertex::new(p[i].x, p[i].y, p[i].z, 0.0, 0.0, c[i]));
    }
    inds.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

// A lit box: each face is shaded by its world normal, lower vertices receive
// ambient-occlusion darkening, and a faint per-box noise tint avoids dead-flat
// surfaces. The result reads with real volume under the baked sun.
fn push_box(verts: &mut Vec<Vertex>, inds: &mut Vec<u16>, b: &Aabb, c: Color) {
    let (mn, mx) = (b.min, b.max);
    let h = (mx.y - mn.y).max(0.001);
    let tint = 0.93 + 0.14 * vnoise(mn * 3.1 + mx);
    let c = Color::new((c.r * tint).min(1.0), (c.g * tint).min(1.0), (c.b * tint).min(1.0), c.a);
    // vertical AO: stronger occlusion near the ground, eases off with height
    let ao_at = |y: f32| 0.5 + 0.5 * ((y - mn.y) / h).clamp(0.0, 1.0).powf(0.6);

    // top (full light, fully lit)
    push_quad(
        verts,
        inds,
        [
            vec3(mn.x, mx.y, mn.z),
            vec3(mx.x, mx.y, mn.z),
            vec3(mx.x, mx.y, mx.z),
            vec3(mn.x, mx.y, mx.z),
        ],
        apply_light(c, vec3(0.0, 1.0, 0.0), 1.0),
    );

    // four sides, each with a top->bottom AO gradient
    let sides: [(Vec3, [Vec3; 4]); 4] = [
        (
            vec3(0.0, 0.0, -1.0),
            [
                vec3(mn.x, mx.y, mn.z),
                vec3(mx.x, mx.y, mn.z),
                vec3(mx.x, mn.y, mn.z),
                vec3(mn.x, mn.y, mn.z),
            ],
        ),
        (
            vec3(0.0, 0.0, 1.0),
            [
                vec3(mx.x, mx.y, mx.z),
                vec3(mn.x, mx.y, mx.z),
                vec3(mn.x, mn.y, mx.z),
                vec3(mx.x, mn.y, mx.z),
            ],
        ),
        (
            vec3(-1.0, 0.0, 0.0),
            [
                vec3(mn.x, mx.y, mx.z),
                vec3(mn.x, mx.y, mn.z),
                vec3(mn.x, mn.y, mn.z),
                vec3(mn.x, mn.y, mx.z),
            ],
        ),
        (
            vec3(1.0, 0.0, 0.0),
            [
                vec3(mx.x, mx.y, mn.z),
                vec3(mx.x, mx.y, mx.z),
                vec3(mx.x, mn.y, mx.z),
                vec3(mx.x, mn.y, mn.z),
            ],
        ),
    ];
    for (n, p) in sides {
        let top = apply_light(c, n, ao_at(mx.y));
        let bot = apply_light(c, n, ao_at(mn.y));
        push_quad_c(verts, inds, p, [top, top, bot, bot]);
    }
}

// unlit box at full colour - for emissive props (lamps, screens, markers)
fn push_box_flat(verts: &mut Vec<Vertex>, inds: &mut Vec<u16>, b: &Aabb, c: Color) {
    let (mn, mx) = (b.min, b.max);
    let faces = [
        [vec3(mn.x, mx.y, mn.z), vec3(mx.x, mx.y, mn.z), vec3(mx.x, mx.y, mx.z), vec3(mn.x, mx.y, mx.z)],
        [vec3(mn.x, mn.y, mn.z), vec3(mx.x, mn.y, mn.z), vec3(mx.x, mn.y, mx.z), vec3(mn.x, mn.y, mx.z)],
        [vec3(mn.x, mx.y, mn.z), vec3(mx.x, mx.y, mn.z), vec3(mx.x, mn.y, mn.z), vec3(mn.x, mn.y, mn.z)],
        [vec3(mx.x, mx.y, mx.z), vec3(mn.x, mx.y, mx.z), vec3(mn.x, mn.y, mx.z), vec3(mx.x, mn.y, mx.z)],
        [vec3(mn.x, mx.y, mx.z), vec3(mn.x, mx.y, mn.z), vec3(mn.x, mn.y, mn.z), vec3(mn.x, mn.y, mx.z)],
        [vec3(mx.x, mx.y, mn.z), vec3(mx.x, mx.y, mx.z), vec3(mx.x, mn.y, mx.z), vec3(mx.x, mn.y, mn.z)],
    ];
    for f in faces {
        push_quad(verts, inds, f, c);
    }
}

// A large sky dome, vertex-coloured into a gradient with a soft sun and a band
// of horizon haze. Drawn first so the world sits inside it.
fn build_sky() -> Mesh {
    let r = 240.0f32;
    let rings = 16;
    let segs = 28;
    let mut v: Vec<Vertex> = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    let s = sun_dir();
    let zenith = vec3(0.16, 0.34, 0.62);
    let horizon = vec3(0.74, 0.80, 0.86);
    for ri in 0..=rings {
        // phi from -0.15 (just below horizon) up to top
        let f = ri as f32 / rings as f32;
        let phi = -0.12 + f * (std::f32::consts::FRAC_PI_2 + 0.12);
        for si in 0..=segs {
            let th = si as f32 / segs as f32 * std::f32::consts::TAU;
            let dir = vec3(phi.cos() * th.cos(), phi.sin(), phi.cos() * th.sin());
            let up = dir.y.clamp(0.0, 1.0);
            let mut col = horizon.lerp(zenith, up.powf(0.65));
            // warm glow toward the sun
            let sd = dir.dot(s).max(0.0);
            col += vec3(0.45, 0.34, 0.16) * sd.powf(7.0);
            col += vec3(0.18, 0.13, 0.07) * sd.powf(2.5);
            v.push(Vertex::new(
                dir.x * r,
                dir.y * r,
                dir.z * r,
                0.0,
                0.0,
                Color::new(col.x.min(1.0), col.y.min(1.0), col.z.min(1.0), 1.0),
            ));
        }
    }
    let stride = segs + 1;
    for ri in 0..rings {
        for si in 0..segs {
            let a = (ri * stride + si) as u16;
            let b = (ri * stride + si + 1) as u16;
            let c = ((ri + 1) * stride + si) as u16;
            let d = ((ri + 1) * stride + si + 1) as u16;
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    Mesh { vertices: v, indices: idx, texture: None }
}

fn aabb(cx: f32, cz: f32, w: f32, d: f32, h: f32, y0: f32) -> Aabb {
    Aabb {
        min: vec3(cx - w / 2.0, y0, cz - d / 2.0),
        max: vec3(cx + w / 2.0, y0 + h, cz + d / 2.0),
    }
}

const SITE_A: Vec2 = Vec2::new(25.0, 0.0);
const SITE_B: Vec2 = Vec2::new(-25.0, 0.0);

fn build_map() -> (Vec<Aabb>, Vec<Mesh>) {
    let wall_col = Color::from_rgba(199, 178, 134, 255);
    let block_col = Color::from_rgba(184, 160, 118, 255);
    let crate_col = Color::from_rgba(158, 113, 68, 255);
    let lintel_col = Color::from_rgba(150, 130, 95, 255);
    let bag_col = Color::from_rgba(125, 120, 92, 255);

    let mut walls: Vec<Aabb> = Vec::new();
    let mut colored: Vec<(Aabb, Color)> = Vec::new();

    let add = |list: &mut Vec<(Aabb, Color)>, walls: &mut Vec<Aabb>, b: Aabb, c: Color| {
        walls.push(b);
        list.push((b, c));
    };

    add(&mut colored, &mut walls, aabb(0.0, -30.5, 82.0, 1.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(0.0, 30.5, 82.0, 1.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-40.5, 0.0, 1.0, 62.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(40.5, 0.0, 1.0, 62.0, 5.0, 0.0), wall_col);

    add(&mut colored, &mut walls, aabb(-36.0, -14.0, 8.0, 18.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(-16.0, -14.0, 7.0, 14.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(36.0, -14.0, 8.0, 18.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(16.0, -14.0, 7.0, 14.0, 4.5, 0.0), block_col);

    add(&mut colored, &mut walls, aabb(-25.75, -21.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(-25.75, -7.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(25.75, -21.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(25.75, -7.0, 12.5, 1.0, 2.1, 2.4), lintel_col);

    add(&mut colored, &mut walls, aabb(-12.0, -10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-12.0, 10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(12.0, -10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(12.0, 10.5, 1.0, 15.0, 4.0, 0.0), wall_col);

    add(&mut colored, &mut walls, aabb(-7.25, 0.0, 8.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(7.25, 0.0, 8.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(0.0, 0.0, 6.0, 1.0, 2.0, 2.5), lintel_col);

    add(&mut colored, &mut walls, aabb(-33.0, 14.0, 15.0, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-17.75, 14.0, 10.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-24.25, 14.0, 2.5, 1.0, 1.6, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(33.0, 14.0, 15.0, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(17.75, 14.0, 10.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(24.25, 14.0, 2.5, 1.0, 1.6, 2.4), lintel_col);

    add(&mut colored, &mut walls, aabb(-29.0, 3.0, 4.0, 1.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(-21.5, -2.0, 1.0, 4.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(28.0, -3.0, 4.0, 1.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(21.5, 2.0, 1.0, 4.0, 1.0, 0.0), bag_col);

    let crates = [
        (-27.0, -2.0),
        (-31.0, 6.0),
        (-24.0, 8.0),
        (24.0, -2.0),
        (27.0, 2.0),
        (31.0, 7.0),
        (22.0, 6.0),
        (0.0, -4.0),
        (-4.0, 6.0),
        (5.0, -10.0),
        (3.0, 10.0),
        (-26.0, -14.0),
        (26.0, -14.0),
        (-8.0, -23.0),
        (8.0, -23.0),
        (-8.0, 23.0),
        (8.0, 23.0),
        (-28.0, 19.0),
        (28.0, 19.0),
    ];
    for (x, z) in crates {
        add(&mut colored, &mut walls, aabb(x, z, 2.0, 2.0, 2.0, 0.0), crate_col);
    }
    add(&mut colored, &mut walls, aabb(0.0, -4.0, 2.0, 2.0, 2.0, 2.0), shade(crate_col, 0.92));
    add(&mut colored, &mut walls, aabb(-27.0, -2.0, 2.0, 2.0, 2.0, 2.0), shade(crate_col, 0.92));
    add(&mut colored, &mut walls, aabb(27.0, 2.0, 2.0, 2.0, 2.0, 2.0), shade(crate_col, 0.92));

    let mut verts = Vec::new();
    let mut inds = Vec::new();
    for (b, c) in &colored {
        push_box(&mut verts, &mut inds, b, *c);
    }

    // ---- decorative, non-colliding dressing baked into the wall mesh ----
    // rooftop trim along the perimeter walls for a city-rooftop silhouette
    let trim_col = Color::from_rgba(150, 132, 96, 255);
    for sx in [-1.0f32, 1.0] {
        push_box(&mut verts, &mut inds, &aabb(sx * 40.5, 0.0, 1.2, 62.0, 0.6, 5.0), trim_col);
    }
    for sz in [-1.0f32, 1.0] {
        push_box(&mut verts, &mut inds, &aabb(0.0, sz * 30.5, 82.0, 1.2, 0.6, 5.0), trim_col);
    }
    // wall-base skirting for grounding
    let skirt = Color::from_rgba(96, 84, 60, 255);
    for sx in [-1.0f32, 1.0] {
        push_box(&mut verts, &mut inds, &aabb(sx * 40.3, 0.0, 0.5, 61.0, 0.5, 0.0), skirt);
    }
    // angled crate stacks / pallets near sites for cover detail
    let wood_dark = Color::from_rgba(120, 84, 48, 255);
    for site in [SITE_A, SITE_B] {
        push_box(&mut verts, &mut inds, &aabb(site.x, site.y - 6.5, 3.0, 1.2, 0.4, 0.0), wood_dark);
        push_box(&mut verts, &mut inds, &aabb(site.x, site.y + 6.5, 3.0, 1.2, 0.4, 0.0), wood_dark);
    }
    // overhead light fixtures (emissive) above each site and mid
    let mut emit = Vec::new();
    let mut emit_i = Vec::new();
    for p in [SITE_A, SITE_B, Vec2::new(0.0, 0.0)] {
        push_box_flat(&mut emit, &mut emit_i, &aabb(p.x, p.y, 1.6, 0.5, 0.25, 4.6), Color::from_rgba(255, 244, 210, 255));
    }
    let wall_mesh = Mesh { vertices: verts, indices: inds, texture: None };
    let emit_mesh = Mesh { vertices: emit, indices: emit_i, texture: None };

    // ---- ground: a lit, tiled concrete floor with painted overlays ----
    let mut gv = Vec::new();
    let mut gi = Vec::new();
    let base = Color::from_rgba(150, 137, 108, 255);
    let tile = 2.0f32;
    let (x0, x1, z0, z1) = (-41.0f32, 41.0, -31.0, 31.0);
    let nx = ((x1 - x0) / tile) as i32;
    let nz = ((z1 - z0) / tile) as i32;
    for iz in 0..nz {
        for ix in 0..nx {
            let ax = x0 + ix as f32 * tile;
            let az = z0 + iz as f32 * tile;
            let bx = ax + tile;
            let bz = az + tile;
            let cx = ax + tile * 0.5;
            let cz = az + tile * 0.5;
            // wear noise + ambient occlusion toward the outer walls
            let n = 0.86 + 0.2 * vnoise(vec3(cx, 0.0, cz));
            let edge = (1.0
                - ((cx.abs() - 36.0).max(0.0) / 5.0).min(1.0) * 0.35)
                * (1.0 - ((cz.abs() - 26.0).max(0.0) / 5.0).min(1.0) * 0.35);
            let c = apply_light(shade(base, n), vec3(0.0, 1.0, 0.0), 0.78 + 0.22 * edge);
            push_quad(&mut gv, &mut gi, [vec3(ax, 0.0, az), vec3(bx, 0.0, az), vec3(bx, 0.0, bz), vec3(ax, 0.0, bz)], c);
        }
    }
    // spawn lanes
    for sx in [-1.0f32, 1.0] {
        push_quad(&mut gv, &mut gi, [
            vec3(sx * 32.0, 0.015, -21.0), vec3(sx * 19.5, 0.015, -21.0),
            vec3(sx * 19.5, 0.015, -7.0), vec3(sx * 32.0, 0.015, -7.0),
        ], Color::from_rgba(118, 116, 110, 255));
    }
    // painted bomb sites with a darker inner pad and a bright border ring
    for (site, _label) in [(SITE_A, 'A'), (SITE_B, 'B')] {
        let ring = Color::from_rgba(220, 150, 60, 255);
        let pad = Color::from_rgba(150, 102, 52, 255);
        // border ring (four strips)
        let r = 4.6;
        let t = 0.35;
        for (a, b, c, d) in [
            (vec3(site.x - r, 0.02, site.y - r), vec3(site.x + r, 0.02, site.y - r), vec3(site.x + r, 0.02, site.y - r + t), vec3(site.x - r, 0.02, site.y - r + t)),
            (vec3(site.x - r, 0.02, site.y + r - t), vec3(site.x + r, 0.02, site.y + r - t), vec3(site.x + r, 0.02, site.y + r), vec3(site.x - r, 0.02, site.y + r)),
            (vec3(site.x - r, 0.02, site.y - r), vec3(site.x - r + t, 0.02, site.y - r), vec3(site.x - r + t, 0.02, site.y + r), vec3(site.x - r, 0.02, site.y + r)),
            (vec3(site.x + r - t, 0.02, site.y - r), vec3(site.x + r, 0.02, site.y - r), vec3(site.x + r, 0.02, site.y + r), vec3(site.x + r - t, 0.02, site.y + r)),
        ] {
            push_quad(&mut gv, &mut gi, [a, b, c, d], ring);
        }
        push_quad(&mut gv, &mut gi, [
            vec3(site.x - 3.4, 0.018, site.y - 3.4), vec3(site.x + 3.4, 0.018, site.y - 3.4),
            vec3(site.x + 3.4, 0.018, site.y + 3.4), vec3(site.x - 3.4, 0.018, site.y + 3.4),
        ], pad);
    }
    // spawn pads, colour-coded per team
    push_quad(&mut gv, &mut gi, [
        vec3(-6.0, 0.02, 23.0), vec3(6.0, 0.02, 23.0), vec3(6.0, 0.02, 29.0), vec3(-6.0, 0.02, 29.0),
    ], Color::from_rgba(96, 122, 180, 255));
    push_quad(&mut gv, &mut gi, [
        vec3(-6.0, 0.02, -29.0), vec3(6.0, 0.02, -29.0), vec3(6.0, 0.02, -23.0), vec3(-6.0, 0.02, -23.0),
    ], Color::from_rgba(184, 132, 80, 255));
    let ground_mesh = Mesh { vertices: gv, indices: gi, texture: None };

    let sky = build_sky();
    (walls, vec![sky, ground_mesh, wall_mesh, emit_mesh])
}

// ---------- navigation ----------

struct Nav {
    w: i32,
    h: i32,
    ox: f32,
    oz: f32,
    step: f32,
    blocked: Vec<bool>,
}

impl Nav {
    fn build(walls: &[Aabb]) -> Nav {
        let (ox, oz, step) = (-38.0, -28.0, 2.0);
        let (w, h) = (39, 29);
        let mut blocked = vec![false; (w * h) as usize];
        for gz in 0..h {
            for gx in 0..w {
                let x = ox + gx as f32 * step;
                let z = oz + gz as f32 * step;
                for wl in walls {
                    if wl.min.y < 1.7
                        && wl.max.y > 0.3
                        && x + 0.65 > wl.min.x
                        && x - 0.65 < wl.max.x
                        && z + 0.65 > wl.min.z
                        && z - 0.65 < wl.max.z
                    {
                        blocked[(gz * w + gx) as usize] = true;
                        break;
                    }
                }
            }
        }
        Nav {
            w,
            h,
            ox,
            oz,
            step,
            blocked,
        }
    }

    fn grid(&self, p: Vec2) -> (i32, i32) {
        (
            ((p.x - self.ox) / self.step).round() as i32,
            ((p.y - self.oz) / self.step).round() as i32,
        )
    }

    fn world(&self, gx: i32, gz: i32) -> Vec2 {
        vec2(self.ox + gx as f32 * self.step, self.oz + gz as f32 * self.step)
    }

    fn is_blocked(&self, gx: i32, gz: i32) -> bool {
        if gx < 0 || gz < 0 || gx >= self.w || gz >= self.h {
            return true;
        }
        self.blocked[(gz * self.w + gx) as usize]
    }

    fn nearest_free(&self, p: Vec2) -> (i32, i32) {
        let (gx, gz) = self.grid(p);
        if !self.is_blocked(gx, gz) {
            return (gx, gz);
        }
        for r in 1i32..6 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    if !self.is_blocked(gx + dx, gz + dz) {
                        return (gx + dx, gz + dz);
                    }
                }
            }
        }
        (gx, gz)
    }

    fn path(&self, from: Vec2, to: Vec2) -> Vec<Vec2> {
        let start = self.nearest_free(from);
        let goal = self.nearest_free(to);
        if start == goal {
            return vec![to];
        }
        let idx = |g: (i32, i32)| (g.1 * self.w + g.0) as usize;
        let mut prev: Vec<i32> = vec![-1; (self.w * self.h) as usize];
        let mut queue = std::collections::VecDeque::new();
        prev[idx(start)] = idx(start) as i32;
        queue.push_back(start);
        let mut found = false;
        while let Some(cur) = queue.pop_front() {
            if cur == goal {
                found = true;
                break;
            }
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = (cur.0 + dx, cur.1 + dz);
                if self.is_blocked(n.0, n.1) || prev[idx(n)] != -1 {
                    continue;
                }
                prev[idx(n)] = idx(cur) as i32;
                queue.push_back(n);
            }
        }
        if !found {
            return vec![];
        }
        let mut path = vec![self.world(goal.0, goal.1)];
        let mut cur = goal;
        while cur != start {
            let p = prev[idx(cur)];
            cur = (p % self.w, p / self.w);
            path.push(self.world(cur.0, cur.1));
        }
        path.reverse();
        let mut out = Vec::new();
        for (i, p) in path.iter().enumerate() {
            if i % 2 == 0 || i == path.len() - 1 {
                out.push(*p);
            }
        }
        out.push(to);
        out
    }
}

// ---------- entities ----------

struct Bot {
    pos: Vec3,
    vy: f32,
    yaw: f32,
    team: Team,
    hp: i32,
    alive: bool,
    name: &'static str,
    goal: Vec2,
    anchor: Vec2,
    path: Vec<Vec2>,
    path_i: usize,
    repath: f32,
    roam_t: f32,
    think: f32,
    target: Option<Ent>,
    can_see: bool,
    lost_t: f32,
    react: f32,
    alert_t: f32,
    burst: i32,
    burst_cd: f32,
    shot_t: f32,
    strafe_t: f32,
    strafe_dir: f32,
    step_t: f32,
    planting: f32,
    defusing: f32,
    anim: f32,
    moving: bool,
    kills: i32,
    deaths: i32,
}

impl Bot {
    fn new(name: &'static str, team: Team, spawn: Vec3) -> Bot {
        Bot {
            pos: spawn,
            vy: 0.0,
            yaw: if team == Team::T { std::f32::consts::PI } else { 0.0 },
            team,
            hp: 100,
            alive: true,
            name,
            goal: vec2(spawn.x, spawn.z),
            anchor: vec2(spawn.x, spawn.z),
            path: vec![],
            path_i: 0,
            repath: 0.0,
            roam_t: 0.0,
            think: gen_range(0.0, 0.15),
            target: None,
            can_see: false,
            lost_t: 0.0,
            react: 0.0,
            alert_t: 0.0,
            burst: 3,
            burst_cd: 0.0,
            shot_t: 0.0,
            strafe_t: 0.0,
            strafe_dir: 1.0,
            step_t: 0.0,
            planting: 0.0,
            defusing: 0.0,
            anim: gen_range(0.0, 6.28),
            moving: false,
            kills: 0,
            deaths: 0,
        }
    }

    fn eye(&self) -> Vec3 {
        self.pos + vec3(0.0, 1.5, 0.0)
    }
}

struct RPlayer {
    id: u8,
    name: String,
    team: Team,
    pos: Vec3,
    yaw: f32,
    hp: i32,
    alive: bool,
    e_held: bool,
    weapon: u8,
    plant_p: f32,
    defuse_p: f32,
    spawn_seq: u32,
    render_pos: Vec3,
    render_yaw: f32,
    anim: f32,
    moving: bool,
    kills: i32,
    deaths: i32,
}

impl RPlayer {
    fn eye(&self) -> Vec3 {
        self.pos + vec3(0.0, EYE, 0.0)
    }
}

#[derive(Clone, Copy)]
struct SimSnap {
    ent: Ent,
    eye: Vec3,
    alive: bool,
    team: Team,
}

struct DrawEnt {
    pos: Vec3,
    yaw: f32,
    team: Team,
    carrier: bool,
    name: String,
    hp: i32,
    phase: f32,
    moving: bool,
}

struct WpnDef {
    name: &'static str,
    dmg: i32,
    auto: bool,
    cooldown: f32,
    mag_size: i32,
    reserve_start: i32,
    reload: f32,
    spread: f32,
    price: i32,
    slot: u8,    // 0 primary, 1 secondary, 2 knife
    recoil: f32, // view-punch magnitude
    sniper: bool,
    hs_mult: i32,
}

// weapon roster (CS-style). Indices used by loadout slots.
const W_KNIFE: usize = 0;
const W_USP: usize = 1;
const W_GLOCK: usize = 2;
const W_DEAGLE: usize = 3;
const W_MP5: usize = 4;
const W_AK: usize = 5;
const W_M4: usize = 6;
const W_AWP: usize = 7;

static WPNS: [WpnDef; 8] = [
    WpnDef { name: "Knife",  dmg: 55,  auto: false, cooldown: 0.45, mag_size: 1,  reserve_start: 0,   reload: 0.0, spread: 0.0,   price: 0,    slot: 2, recoil: 0.0,   sniper: false, hs_mult: 2 },
    WpnDef { name: "USP-S",  dmg: 34,  auto: false, cooldown: 0.16, mag_size: 12, reserve_start: 36,  reload: 2.1, spread: 0.006, price: 0,    slot: 1, recoil: 0.010, sniper: false, hs_mult: 4 },
    WpnDef { name: "Glock",  dmg: 28,  auto: false, cooldown: 0.13, mag_size: 20, reserve_start: 40,  reload: 2.0, spread: 0.008, price: 0,    slot: 1, recoil: 0.008, sniper: false, hs_mult: 4 },
    WpnDef { name: "Deagle", dmg: 58,  auto: false, cooldown: 0.27, mag_size: 7,  reserve_start: 35,  reload: 2.2, spread: 0.010, price: 700,  slot: 1, recoil: 0.030, sniper: false, hs_mult: 4 },
    WpnDef { name: "MP5",    dmg: 27,  auto: true,  cooldown: 0.075,mag_size: 30, reserve_start: 120, reload: 2.3, spread: 0.012, price: 1500, slot: 0, recoil: 0.009, sniper: false, hs_mult: 4 },
    WpnDef { name: "AK-47",  dmg: 36,  auto: true,  cooldown: 0.099,mag_size: 30, reserve_start: 90,  reload: 2.4, spread: 0.009, price: 2700, slot: 0, recoil: 0.020, sniper: false, hs_mult: 4 },
    WpnDef { name: "M4A1-S", dmg: 33,  auto: true,  cooldown: 0.090,mag_size: 30, reserve_start: 90,  reload: 2.3, spread: 0.008, price: 2900, slot: 0, recoil: 0.015, sniper: false, hs_mult: 4 },
    WpnDef { name: "AWP",    dmg: 115, auto: false, cooldown: 1.20, mag_size: 5,  reserve_start: 20,  reload: 3.6, spread: 0.001, price: 4750, slot: 0, recoil: 0.055, sniper: true,  hs_mult: 2 },
];

#[derive(Clone, Copy)]
struct WpnState {
    mag: i32,
    reserve: i32,
    reload_t: f32,
    cd: f32,
}

struct Bomb {
    pos: Vec3,
    t: f32,
    beep_t: f32,
}

// ---------- effects ----------

struct Particle {
    pos: Vec3,
    vel: Vec3,
    life: f32,
    max: f32,
    size: f32,
    col: Color,
    grav: f32,
    drag: f32,
}

struct Decal {
    pos: Vec3,
    n: Vec3,
    size: f32,
    life: f32,
    col: Color,
}

struct Corpse {
    pos: Vec3,
    yaw: f32,
    team: Team,
    t: f32,
}

struct DmgNum {
    pos: Vec3,
    amount: i32,
    life: f32,
    head: bool,
    vel: Vec3,
}

enum Role {
    Solo,
    Host(net::HostNet),
    Client(net::ClientNet),
}

// client-side view of the world from snapshots
struct CState {
    my_id: u8,
    welcomed: bool,
    snap: Option<Snapshot>,
    step_acc: std::collections::HashMap<u8, (Vec3, f32)>,
    spawn_seq: u32,
    beep_t: f32,
    prev_hp: i32,
    prev_alive: bool,
    disconnect: Option<String>,
}

struct Game {
    walls: Vec<Aabb>,
    meshes: Vec<Mesh>,
    nav: Nav,
    snd: Sounds,

    role: Role,
    my_name: String,
    ff: bool,
    bot_fill: i32,
    ticket: Option<String>,
    cstate: CState,
    remotes: Vec<RPlayer>,
    ev_buf: Vec<Event>,
    snap_timer: f32,
    input_timer: f32,

    player_team: Team,
    ppos: Vec3,
    pvel: Vec3,
    yaw: f32,
    pitch: f32,
    php: i32,
    palive: bool,
    on_ground: bool,
    cur: usize,
    loadout: [usize; 3],
    wpn: [WpnState; 3],
    spread_add: f32,
    bob: f32,
    switch_t: f32,
    muzzle_t: f32,
    defuse_t: f32,
    plant_t: f32,
    step_t: f32,
    spec_i: usize,
    // recoil / view feel
    punch: Vec2,
    recoil_i: i32,
    land_t: f32,
    zoom: f32,
    // economy
    money: i32,
    armor: i32,
    helmet: bool,
    kit: bool,
    buy_open: bool,
    my_kills: i32,
    my_deaths: i32,
    pending_buy_primary: usize,
    pending_buy_secondary: usize,
    show_scores: bool,
    menu_t: f32,

    bots: Vec<Bot>,

    bomb: Option<Bomb>,
    defused: bool,
    carrier: Option<Ent>,
    dropped: Option<Vec3>,
    site: Vec2,

    phase: Phase,
    phase_t: f32,
    round_time: f32,
    round: i32,
    score_ct: i32,
    score_t: i32,
    match_over: bool,

    msg: String,
    msg_t: f32,
    sub: String,
    sub_t: f32,
    killfeed: Vec<(String, Color, f32)>,
    tracers: Vec<(Vec3, Vec3, f32)>,
    flashes: Vec<(Vec3, f32)>,
    explosion: Option<(Vec3, f32)>,
    particles: Vec<Particle>,
    decals: Vec<Decal>,
    corpses: Vec<Corpse>,
    dmg_nums: Vec<DmgNum>,
    dmg_dir: Vec<(f32, f32)>, // (world yaw of damage source, life)
    vign: f32,
    hitm: f32,
    hitk: bool, // last hitmarker was a kill
    shake: f32,

    dbg_open: bool,
    dbg_god: bool,
    dbg_noclip: bool,
    dbg_esp: bool,
    dbg_paths: bool,
    dbg_freeze: bool,
    dbg_uncap: bool,

    grabbed: bool,
    last_mouse: Vec2,
}

struct DmgEvent {
    target: Ent,
    dmg: i32,
    killer: String,
    killer_team: Team,
    src: Vec3,
}

impl Game {
    fn is_authority(&self) -> bool {
        !matches!(self.role, Role::Client(_))
    }

    fn wdef(&self) -> &'static WpnDef {
        &WPNS[self.loadout[self.cur]]
    }

    fn refill_loadout(&mut self) {
        for slot in 0..3 {
            let d = &WPNS[self.loadout[slot]];
            self.wpn[slot] = WpnState {
                mag: d.mag_size,
                reserve: d.reserve_start,
                reload_t: 0.0,
                cd: 0.0,
            };
        }
    }

    fn player_eye(&self) -> Vec3 {
        self.ppos + vec3(0.0, EYE, 0.0)
    }

    fn look_dir(&self) -> Vec3 {
        // recoil view-punch rides on top of the aim so sprays climb realistically
        let yaw = self.yaw + self.punch.x;
        let pitch = (self.pitch + self.punch.y).clamp(-1.5, 1.5);
        vec3(pitch.cos() * -yaw.sin(), pitch.sin(), pitch.cos() * -yaw.cos())
    }

    fn flat_forward(&self) -> Vec3 {
        vec3(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    fn flat_right(&self) -> Vec3 {
        vec3(self.yaw.cos(), 0.0, -self.yaw.sin())
    }

    fn show_msg(&mut self, m: &str, t: f32) {
        self.msg = m.to_string();
        self.msg_t = t;
    }

    fn show_sub(&mut self, m: &str, t: f32) {
        self.sub = m.to_string();
        self.sub_t = t;
    }

    fn add_kill(&mut self, killer: &str, killer_team: Team, victim: &str) {
        let c = if killer_team == Team::Ct {
            Color::from_rgba(120, 180, 255, 255)
        } else {
            Color::from_rgba(255, 180, 110, 255)
        };
        self.killfeed.push((format!("{killer}  ⮞  {victim}"), c, 5.0));
        if self.killfeed.len() > 6 {
            self.killfeed.remove(0);
        }
        // scoreboard + economy bookkeeping
        if killer != victim {
            if killer == "You" {
                self.my_kills += 1;
                self.money = (self.money + 300).min(16000);
            } else if let Some(b) = self.bots.iter_mut().find(|b| b.name == killer) {
                b.kills += 1;
            } else if let Some(r) = self.remotes.iter_mut().find(|r| r.name == killer) {
                r.kills += 1;
            }
        }
        if victim == "You" {
            self.my_deaths += 1;
        } else if let Some(b) = self.bots.iter_mut().find(|b| b.name == victim) {
            b.deaths += 1;
        } else if let Some(r) = self.remotes.iter_mut().find(|r| r.name == victim) {
            r.deaths += 1;
        }
        if self.is_authority() {
            self.ev_buf.push(Event::Kill {
                killer: killer.to_string(),
                killer_ct: killer_team == Team::Ct,
                victim: victim.to_string(),
            });
        }
    }

    // ---------- round setup (authority) ----------

    fn humans_on(&self, team: Team) -> usize {
        let mut n = if self.player_team == team { 1 } else { 0 };
        n += self.remotes.iter().filter(|r| r.team == team).count();
        n
    }

    fn rebuild_bots(&mut self) {
        self.bots.clear();
        let mut name_i = 0;
        for team in [Team::T, Team::Ct] {
            let humans = self.humans_on(team);
            let target = (self.bot_fill as usize).max(humans).max(1);
            let n_bots = target.saturating_sub(humans).min(SPAWN_X.len() - humans);
            let z = if team == Team::T { -26.0 } else { 26.0 };
            for k in 0..n_bots {
                let x = SPAWN_X[(humans + k).min(SPAWN_X.len() - 1)];
                self.bots.push(Bot::new(BOT_NAMES[name_i % BOT_NAMES.len()], team, vec3(x, 0.0, z)));
                name_i += 1;
            }
        }
    }

    fn team_spawn(&self, team: Team, slot: usize) -> (Vec3, f32) {
        let z = if team == Team::T { -26.0 } else { 26.0 };
        let yaw = if team == Team::T { std::f32::consts::PI } else { 0.0 };
        (vec3(SPAWN_X[slot.min(SPAWN_X.len() - 1)], 0.0, z), yaw)
    }

    fn start_round(&mut self) {
        if self.match_over {
            self.score_ct = 0;
            self.score_t = 0;
            self.round = 0;
            self.match_over = false;
        }
        self.round += 1;

        // host player gets slot 0 of own team; remotes get following slots
        let (sp, syaw) = self.team_spawn(self.player_team, 0);
        self.ppos = sp;
        self.yaw = syaw;
        self.pvel = Vec3::ZERO;
        self.pitch = 0.0;
        self.php = 100;
        self.palive = true;
        self.cur = 0;
        // match start: hand out default pistols + reset economy
        if self.round == 1 {
            let def_primary = if self.player_team == Team::T { W_GLOCK } else { W_USP };
            self.loadout = [def_primary, def_primary, W_KNIFE];
            // slot 0 holds a pistol on the pistol round (no rifle yet)
            self.money = 800;
            self.armor = 0;
            self.helmet = false;
            self.kit = false;
            self.my_kills = 0;
            self.my_deaths = 0;
            self.pending_buy_primary = if self.player_team == Team::T { W_AK } else { W_M4 };
            self.pending_buy_secondary = if self.player_team == Team::T { W_GLOCK } else { W_USP };
        }
        self.refill_loadout();
        self.cur = if WPNS[self.loadout[0]].slot == 0 { 0 } else { 1 };
        self.spread_add = 0.0;
        self.defuse_t = 0.0;
        self.plant_t = 0.0;
        self.punch = Vec2::ZERO;
        self.zoom = 0.0;
        self.buy_open = false;
        self.vign = 0.0;

        // remotes
        let mut slot_t = if self.player_team == Team::T { 1 } else { 0 };
        let mut slot_ct = if self.player_team == Team::Ct { 1 } else { 0 };
        let mut spawns: Vec<(u8, Vec3, f32)> = Vec::new();
        for r in &mut self.remotes {
            let slot = if r.team == Team::T {
                slot_t += 1;
                slot_t - 1
            } else {
                slot_ct += 1;
                slot_ct - 1
            };
            let z = if r.team == Team::T { -26.0 } else { 26.0 };
            let yaw = if r.team == Team::T { std::f32::consts::PI } else { 0.0 };
            let pos = vec3(SPAWN_X[slot.min(SPAWN_X.len() - 1)], 0.0, z);
            r.pos = pos;
            r.render_pos = pos;
            r.hp = 100;
            r.alive = true;
            r.plant_p = 0.0;
            r.defuse_p = 0.0;
            r.spawn_seq += 1;
            spawns.push((r.id, pos, yaw));
        }
        if let Role::Host(net) = &self.role {
            for (id, pos, yaw) in spawns {
                let seq = self.remotes.iter().find(|r| r.id == id).unwrap().spawn_seq;
                net.send_to(id, S2C::Spawn { pos: a3(pos), yaw, seq });
            }
        }

        self.rebuild_bots();

        self.bomb = None;
        self.defused = false;
        self.dropped = None;
        self.explosion = None;
        self.site = if gen_range(0, 2) == 0 { SITE_A } else { SITE_B };

        let mut t_ents: Vec<Ent> = Vec::new();
        if self.player_team == Team::T {
            t_ents.push(Ent::Me);
        }
        for r in &self.remotes {
            if r.team == Team::T {
                t_ents.push(Ent::Remote(r.id));
            }
        }
        for (i, b) in self.bots.iter().enumerate() {
            if b.team == Team::T {
                t_ents.push(Ent::Bot(i));
            }
        }
        self.carrier = if t_ents.is_empty() {
            None
        } else {
            Some(t_ents[gen_range(0, t_ents.len() as i32) as usize])
        };

        let site = self.site;
        let mut ct_count = 0;
        for b in self.bots.iter_mut() {
            match b.team {
                Team::T => {
                    b.goal = site + vec2(gen_range(-4.0, 4.0), gen_range(-4.0, 4.0));
                }
                Team::Ct => {
                    let s = if ct_count % 2 == 0 { SITE_A } else { SITE_B };
                    ct_count += 1;
                    b.anchor = s;
                    b.goal = s + vec2(gen_range(-5.0, 5.0), gen_range(-5.0, 5.0));
                }
            }
        }

        self.round_time = ROUND_TIME;
        self.phase = Phase::Freeze;
        self.phase_t = FREEZE_TIME;
        self.show_msg(&format!("Round {}", self.round), 2.5);
        self.ev_buf.push(Event::RoundMsg { round: self.round });
        if self.carrier == Some(Ent::Me) {
            self.show_sub("You have the C4 - hold E at a site to plant", 6.0);
        }
    }

    fn end_round(&mut self, winner: Team, reason: &str) {
        if self.phase == Phase::Post {
            return;
        }
        match winner {
            Team::Ct => self.score_ct += 1,
            Team::T => self.score_t += 1,
        }
        self.phase = Phase::Post;
        self.phase_t = 4.0;
        // round-end economy so the buy menu stays meaningful
        let won = winner == self.player_team;
        self.money = (self.money + if won { 3250 } else { 1900 }).min(16000);
        let who = if winner == Team::Ct {
            "Counter-Terrorists win"
        } else {
            "Terrorists win"
        };
        self.show_msg(&format!("{who} - {reason}"), 4.0);
        self.ev_buf.push(Event::RoundEnd {
            ct_won: winner == Team::Ct,
            reason: reason.to_string(),
        });
        if self.score_ct >= WIN_SCORE || self.score_t >= WIN_SCORE {
            self.match_over = true;
            self.phase_t = 6.0;
            let ct_won = self.score_ct >= WIN_SCORE;
            let player_won = (ct_won && self.player_team == Team::Ct)
                || (!ct_won && self.player_team == Team::T);
            if player_won {
                self.show_msg("MATCH WON - your team takes it!", 6.0);
                play(&self.snd.flash, 0.6);
            } else {
                self.show_msg("MATCH LOST - better luck next time", 6.0);
            }
        }
    }

    // ---------- shared local player movement / weapons ----------

    fn local_phase_live(&self) -> bool {
        if self.is_authority() {
            self.phase == Phase::Live
        } else {
            self.cstate.snap.as_ref().map_or(false, |s| s.phase == 1)
        }
    }

    fn player_update(&mut self, dt: f32) {
        if !self.palive {
            return;
        }
        if self.buy_open {
            return;
        }
        let frozen = !self.local_phase_live();

        // scope (ADS) for sniper rifles on right mouse
        let want_zoom = self.wdef().sniper
            && is_mouse_button_down(MouseButton::Right)
            && !frozen
            && self.wpn[self.cur].reload_t <= 0.0;
        let zt = if want_zoom { 1.0 } else { 0.0 };
        self.zoom += (zt - self.zoom) * (dt * 16.0).min(1.0);

        if self.dbg_noclip {
            let mut wish = Vec3::ZERO;
            if is_key_down(KeyCode::W) {
                wish += self.look_dir();
            }
            if is_key_down(KeyCode::S) {
                wish -= self.look_dir();
            }
            if is_key_down(KeyCode::D) {
                wish += self.flat_right();
            }
            if is_key_down(KeyCode::A) {
                wish -= self.flat_right();
            }
            if is_key_down(KeyCode::Space) {
                wish += vec3(0.0, 1.0, 0.0);
            }
            if is_key_down(KeyCode::LeftControl) {
                wish -= vec3(0.0, 1.0, 0.0);
            }
            if wish.length_squared() > 0.0 {
                self.ppos += wish.normalize() * 14.0 * dt;
            }
            self.pvel = Vec3::ZERO;
        } else {
            let mut wish = Vec3::ZERO;
            if !frozen {
                if is_key_down(KeyCode::W) {
                    wish += self.flat_forward();
                }
                if is_key_down(KeyCode::S) {
                    wish -= self.flat_forward();
                }
                if is_key_down(KeyCode::D) {
                    wish += self.flat_right();
                }
                if is_key_down(KeyCode::A) {
                    wish -= self.flat_right();
                }
            }
            let walking = is_key_down(KeyCode::LeftShift);
            let speed = if walking { WALK_SPEED } else { PLAYER_SPEED };
            if wish.length_squared() > 0.0 {
                wish = wish.normalize() * speed;
            }
            if !frozen && is_key_down(KeyCode::Space) && self.on_ground {
                self.pvel.y = JUMP_VEL;
            }
            self.pvel.x = wish.x;
            self.pvel.z = wish.z;
            self.pvel.y -= GRAVITY * dt;
            let mut vy = self.pvel.y;
            let delta = self.pvel * dt;
            self.on_ground = move_collide(&mut self.ppos, &mut vy, PLAYER_R, PLAYER_H, delta, &self.walls);
            self.pvel.y = vy;

            let moving = wish.length_squared() > 1.0;
            self.bob += dt * if moving { 9.0 } else { 0.0 };
            self.spread_add = (self.spread_add - dt * 0.06).max(0.0)
                + if moving { dt * 0.012 } else { 0.0 };
            self.spread_add = self.spread_add.min(0.025);

            if moving && !walking && self.on_ground {
                self.step_t -= dt;
                if self.step_t <= 0.0 {
                    self.step_t = 0.34;
                    play(&self.snd.step, 0.22);
                }
            } else {
                self.step_t = 0.1;
            }
        }

        if self.switch_t > 0.0 {
            self.switch_t -= dt;
        }
        let switch_keys = [(KeyCode::Key1, 0usize), (KeyCode::Key2, 1), (KeyCode::Key3, 2)];
        for (k, slot) in switch_keys {
            if is_key_pressed(k) && self.cur != slot {
                self.cur = slot;
                self.switch_t = 0.4;
                self.wpn[slot].reload_t = 0.0;
                self.zoom = 0.0;
                play(&self.snd.draw, 0.4);
            }
        }
        let c = self.cur;
        let wi = self.loadout[c];
        if self.wpn[c].cd > 0.0 {
            self.wpn[c].cd -= dt;
        }
        if self.wpn[c].reload_t > 0.0 {
            self.wpn[c].reload_t -= dt;
            if self.wpn[c].reload_t <= 0.0 {
                let need = WPNS[wi].mag_size - self.wpn[c].mag;
                let take = need.min(self.wpn[c].reserve);
                self.wpn[c].mag += take;
                self.wpn[c].reserve -= take;
            }
        }
        if is_key_pressed(KeyCode::R)
            && WPNS[wi].slot != 2
            && self.wpn[c].reload_t <= 0.0
            && self.wpn[c].mag < WPNS[wi].mag_size
            && self.wpn[c].reserve > 0
        {
            self.wpn[c].reload_t = WPNS[wi].reload;
            self.zoom = 0.0;
            play(&self.snd.reload, 0.5);
        }

        let busy_e = self.local_defusing() || self.local_planting();
        let is_knife = WPNS[wi].slot == 2;
        let want_fire = if WPNS[wi].auto {
            is_mouse_button_down(MouseButton::Left)
        } else {
            is_mouse_button_pressed(MouseButton::Left)
        };
        if want_fire
            && !frozen
            && !busy_e
            && self.switch_t <= 0.0
            && self.wpn[c].cd <= 0.0
            && self.wpn[c].reload_t <= 0.0
        {
            if is_knife || self.wpn[c].mag > 0 {
                self.fire_weapon();
            } else if is_mouse_button_pressed(MouseButton::Left) {
                play(&self.snd.dryfire, 0.4);
                if self.wpn[c].reserve > 0 {
                    self.wpn[c].reload_t = WPNS[wi].reload;
                }
            }
        }

        // recoil resets to zero once the trigger is released
        if !is_mouse_button_down(MouseButton::Left) {
            self.recoil_i = 0;
        }

        // T player: pick up dropped bomb (authority handles this in sim; client trusts host)
        if self.is_authority() && self.player_team == Team::T && self.carrier.is_none() {
            if let Some(dp) = self.dropped {
                if self.ppos.distance(dp) < 1.6 {
                    self.carrier = Some(Ent::Me);
                    self.dropped = None;
                    self.show_sub("Picked up the C4", 3.0);
                }
            }
        }
    }

    fn i_am_carrier(&self) -> bool {
        if self.is_authority() {
            self.carrier == Some(Ent::Me)
        } else {
            self.cstate
                .snap
                .as_ref()
                .and_then(|s| s.players.iter().find(|p| p.id == self.cstate.my_id))
                .map_or(false, |p| p.carrier)
        }
    }

    fn bomb_view(&self) -> Option<(Vec3, f32, bool)> {
        if self.is_authority() {
            self.bomb.as_ref().map(|b| (b.pos, b.t, self.defused))
        } else {
            self.cstate
                .snap
                .as_ref()
                .and_then(|s| s.bomb)
                .map(|(p, t, d)| (v3(p), t, d))
        }
    }

    fn local_defusing(&self) -> bool {
        if self.player_team != Team::Ct || !self.palive || !self.local_phase_live() {
            return false;
        }
        if let Some((bp, _, defused)) = self.bomb_view() {
            !defused && is_key_down(KeyCode::E) && self.ppos.distance(bp) < 2.4
        } else {
            false
        }
    }

    fn local_planting(&self) -> bool {
        self.player_team == Team::T
            && self.i_am_carrier()
            && self.palive
            && self.local_phase_live()
            && self.bomb_view().is_none()
            && is_key_down(KeyCode::E)
            && self.near_any_site()
    }

    fn near_any_site(&self) -> bool {
        let p = vec2(self.ppos.x, self.ppos.z);
        p.distance(SITE_A) < 4.5 || p.distance(SITE_B) < 4.5
    }

    // raycast against bots + remote players (authority) or snapshot entities (client)
    fn fire_weapon(&mut self) {
        let c = self.cur;
        let wi = self.loadout[c];
        let def = &WPNS[wi];
        let is_knife = def.slot == 2;
        let range = if is_knife { 2.3 } else { 1000.0 };
        if !is_knife {
            self.wpn[c].mag -= 1;
        }
        self.wpn[c].cd = def.cooldown;

        // inaccuracy: base + movement; snipers are wildly inaccurate unless scoped
        let mut spread = def.spread + self.spread_add;
        if def.sniper && self.zoom < 0.5 {
            spread += 0.06;
        }
        self.muzzle_t = if is_knife { 0.0 } else { 0.05 };
        self.shake = (0.006 + def.recoil * 0.4).min(0.05);

        // recoil view-punch: kick up, drift sideways, accumulates on auto fire
        if !is_knife {
            let climb = def.recoil * (1.0 + (self.recoil_i as f32) * 0.06).min(2.2);
            self.punch.y += climb;
            self.punch.x += gen_range(-1.0, 1.0) * def.recoil * 0.5;
            self.recoil_i += 1;
            self.spread_add = (self.spread_add + def.recoil * 0.25).min(0.05);
            self.zoom *= 0.0_f32.max(if def.sniper { 0.0 } else { 1.0 });
        }

        let eye = self.player_eye();
        let dir = self.look_dir();
        let right = self.flat_right();
        let up = right.cross(dir).normalize();
        let d = (dir + right * gen_range(-spread, spread) + up * gen_range(-spread, spread)).normalize();

        let wall_t = nearest_wall_hit(eye, d, range, &self.walls);
        let mut best_t = wall_t;
        let mut hit: Option<(Ent, bool)> = None;

        let test = |pos: Vec3, ent: Ent, best_t: &mut f32, hit: &mut Option<(Ent, bool)>| {
            let head = Aabb {
                min: pos + vec3(-0.18, 1.4, -0.18),
                max: pos + vec3(0.18, 1.75, 0.18),
            };
            let body = Aabb {
                min: pos + vec3(-0.35, 0.0, -0.35),
                max: pos + vec3(0.35, 1.4, 0.35),
            };
            if let Some(t) = ray_aabb(eye, d, &head) {
                if t < *best_t {
                    *best_t = t;
                    *hit = Some((ent, true));
                }
            }
            if let Some(t) = ray_aabb(eye, d, &body) {
                if t < *best_t {
                    *best_t = t;
                    *hit = Some((ent, false));
                }
            }
        };

        if self.is_authority() {
            for (i, b) in self.bots.iter().enumerate() {
                if !b.alive || (!self.ff && b.team == self.player_team) {
                    continue;
                }
                test(b.pos, Ent::Bot(i), &mut best_t, &mut hit);
            }
            for r in &self.remotes {
                if !r.alive || (!self.ff && r.team == self.player_team) {
                    continue;
                }
                test(r.pos, Ent::Remote(r.id), &mut best_t, &mut hit);
            }
        } else if let Some(snap) = &self.cstate.snap {
            for (i, b) in snap.bots.iter().enumerate() {
                if !b.alive || (!self.ff && Team::from_t(b.team_t) == self.player_team) {
                    continue;
                }
                test(v3(b.pos), Ent::Bot(i), &mut best_t, &mut hit);
            }
            for p in &snap.players {
                if p.id == self.cstate.my_id
                    || !p.alive
                    || (!self.ff && Team::from_t(p.team_t) == self.player_team)
                {
                    continue;
                }
                test(v3(p.pos), Ent::Remote(p.id), &mut best_t, &mut hit);
            }
        }

        let muzzle = eye + dir * 0.4 + right * 0.14 - up * 0.1;
        let end = eye + d * best_t;
        if is_knife {
            play(&self.snd.knife, 0.6);
        } else {
            self.tracers.push((muzzle, end, 0.05));
            self.flashes.push((muzzle, 0.04));
            self.spawn_casing(eye, right, up);
            let g = match wi {
                W_AK => &self.snd.ak,
                W_M4 => &self.snd.m4,
                W_MP5 => &self.snd.mp5,
                W_DEAGLE => &self.snd.deagle,
                W_AWP => &self.snd.awp,
                _ => &self.snd.pistol,
            };
            play(g, 0.9);
        }

        if self.is_authority() {
            self.ev_buf.push(Event::Tracer {
                from: a3(muzzle),
                to: a3(end),
            });
            for b in &mut self.bots {
                if b.alive && b.pos.distance(eye) < 28.0 {
                    b.alert_t = b.alert_t.max(3.0);
                }
            }
        }

        let pistol_flag = def.slot == 1;
        if let Some((ent, headshot)) = hit {
            let dmg = if headshot { def.dmg * def.hs_mult } else { def.dmg };
            self.hitm = 0.12;
            play(if headshot { &self.snd.headshot } else { &self.snd.hit }, 0.5);
            self.spawn_blood(end, d);
            self.add_dmg_num(end, dmg, headshot);
            self.hitk = false;
            if self.is_authority() {
                self.hitk = self.lethal_to(ent, dmg);
                self.apply_damage(ent, dmg, "You".to_string(), self.player_team);
            } else if let Role::Client(net) = &self.role {
                let claim = match ent {
                    Ent::Bot(i) => Some(HitClaim::Bot { idx: i as u8, dmg }),
                    Ent::Remote(id) => Some(HitClaim::Player { id, dmg }),
                    Ent::Me => None,
                };
                net.send(C2S::Shot(ShotMsg { from: a3(muzzle), to: a3(end), pistol: pistol_flag, hit: claim }));
            }
        } else {
            if !is_knife && best_t < range - 0.5 {
                let a = d.abs();
                let n = if a.x >= a.y && a.x >= a.z {
                    vec3(-d.x.signum(), 0.0, 0.0)
                } else if a.y >= a.z {
                    vec3(0.0, -d.y.signum(), 0.0)
                } else {
                    vec3(0.0, 0.0, -d.z.signum())
                };
                self.spawn_impact(end, n);
            }
            if let Role::Client(net) = &self.role {
                net.send(C2S::Shot(ShotMsg { from: a3(muzzle), to: a3(end), pistol: pistol_flag, hit: None }));
            }
        }
    }

    // does `dmg` finish this target? used to flag the hitmarker as a kill
    fn lethal_to(&self, ent: Ent, dmg: i32) -> bool {
        match ent {
            Ent::Bot(i) => self.bots.get(i).map_or(false, |b| b.alive && b.hp <= dmg),
            Ent::Remote(id) => self
                .remotes
                .iter()
                .find(|r| r.id == id)
                .map_or(false, |r| r.alive && r.hp <= dmg),
            Ent::Me => false,
        }
    }

    // ---------- effect spawners ----------

    fn spawn_impact(&mut self, p: Vec3, n: Vec3) {
        play(&self.snd.hit, dist_vol(self.player_eye(), p, 0.18));
        // sparks
        for _ in 0..7 {
            let v = (n + vec3(gen_range(-0.8, 0.8), gen_range(-0.2, 1.0), gen_range(-0.8, 0.8))).normalize() * gen_range(2.0, 6.0);
            self.particles.push(Particle {
                pos: p + n * 0.02,
                vel: v,
                life: gen_range(0.15, 0.35),
                max: 0.35,
                size: 0.025,
                col: Color::from_rgba(255, 220, 140, 255),
                grav: 9.0,
                drag: 1.0,
            });
        }
        // dust puff
        for _ in 0..3 {
            self.particles.push(Particle {
                pos: p + n * 0.05,
                vel: n * gen_range(0.4, 1.0) + vec3(gen_range(-0.3, 0.3), gen_range(0.2, 0.6), gen_range(-0.3, 0.3)),
                life: gen_range(0.4, 0.8),
                max: 0.8,
                size: 0.18,
                col: Color::from_rgba(150, 140, 120, 120),
                grav: -0.3,
                drag: 2.2,
            });
        }
        if self.decals.len() < 200 {
            self.decals.push(Decal {
                pos: p + n * 0.015,
                n,
                size: gen_range(0.10, 0.18),
                life: 30.0,
                col: Color::from_rgba(30, 26, 22, 230),
            });
        }
    }

    fn spawn_blood(&mut self, p: Vec3, dir: Vec3) {
        for _ in 0..10 {
            let v = dir * gen_range(1.0, 3.0) + vec3(gen_range(-1.5, 1.5), gen_range(-0.5, 1.5), gen_range(-1.5, 1.5));
            self.particles.push(Particle {
                pos: p,
                vel: v,
                life: gen_range(0.2, 0.5),
                max: 0.5,
                size: 0.04,
                col: Color::from_rgba(150, 12, 12, 255),
                grav: 11.0,
                drag: 0.8,
            });
        }
        if self.decals.len() < 200 {
            self.decals.push(Decal {
                pos: vec3(p.x + dir.x * 0.5, 0.02, p.z + dir.z * 0.5),
                n: vec3(0.0, 1.0, 0.0),
                size: gen_range(0.3, 0.6),
                life: 25.0,
                col: Color::from_rgba(110, 10, 10, 200),
            });
        }
    }

    fn spawn_casing(&mut self, eye: Vec3, right: Vec3, up: Vec3) {
        self.particles.push(Particle {
            pos: eye + right * 0.25 - up * 0.1,
            vel: right * gen_range(1.5, 2.8) + up * gen_range(1.0, 2.0) + self.flat_forward() * gen_range(-0.4, 0.4),
            life: 0.7,
            max: 0.7,
            size: 0.03,
            col: Color::from_rgba(210, 170, 70, 255),
            grav: 14.0,
            drag: 0.4,
        });
    }

    fn add_dmg_num(&mut self, p: Vec3, amount: i32, head: bool) {
        self.dmg_nums.push(DmgNum {
            pos: p + vec3(0.0, 0.2, 0.0),
            amount,
            life: 0.9,
            head,
            vel: vec3(gen_range(-0.3, 0.3), 1.2, gen_range(-0.3, 0.3)),
        });
    }

    fn add_dmg_dir(&mut self, from: Vec3) {
        let d = from - self.player_eye();
        let yaw = (-d.x).atan2(-d.z);
        self.dmg_dir.push((yaw, 1.4));
    }

    // particle physics, decals/corpses ageing, recoil recovery
    fn tick_fx(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.life -= dt;
            p.vel.y -= p.grav * dt;
            let k = (1.0 - p.drag * dt).clamp(0.0, 1.0);
            p.vel *= k;
            p.pos += p.vel * dt;
            if p.pos.y < 0.02 {
                p.pos.y = 0.02;
                p.vel.x *= 0.4;
                p.vel.z *= 0.4;
                p.vel.y = -p.vel.y * 0.25;
            }
        }
        self.particles.retain(|p| p.life > 0.0);
        for d in &mut self.decals {
            d.life -= dt;
        }
        self.decals.retain(|d| d.life > 0.0);
        for c in &mut self.corpses {
            c.t -= dt;
        }
        self.corpses.retain(|c| c.t > 0.0);
        for d in &mut self.dmg_nums {
            d.life -= dt;
            d.pos += d.vel * dt;
            d.vel *= (1.0 - 2.0 * dt).max(0.0);
        }
        self.dmg_nums.retain(|d| d.life > 0.0);
        for d in &mut self.dmg_dir {
            d.1 -= dt;
        }
        self.dmg_dir.retain(|d| d.1 > 0.0);
        // recoil view-punch eases back to centre
        self.punch = self.punch.lerp(Vec2::ZERO, (dt * 9.0).min(1.0));
        self.land_t = (self.land_t - dt).max(0.0);
    }

    // ---------- authority: damage / deaths ----------

    fn apply_damage(&mut self, target: Ent, dmg: i32, killer: String, killer_team: Team) {
        if self.phase != Phase::Live {
            return;
        }
        let dmg = dmg.clamp(0, 150);
        match target {
            Ent::Me => {
                if !self.palive || self.dbg_god {
                    return;
                }
                if !self.ff && killer_team == self.player_team {
                    return;
                }
                // kevlar soaks part of the hit and wears down
                let mut dmg = dmg;
                if self.armor > 0 {
                    let absorbed = (dmg as f32 * 0.5) as i32;
                    self.armor = (self.armor - absorbed / 2).max(0);
                    dmg -= absorbed;
                }
                self.php -= dmg;
                self.vign = (self.vign + 0.45).min(1.0);
                self.shake = 0.025;
                if self.php <= 0 {
                    self.php = 0;
                    self.player_die(&killer, killer_team);
                }
            }
            Ent::Remote(id) => {
                let Some(ri) = self.remotes.iter().position(|r| r.id == id) else {
                    return;
                };
                if !self.remotes[ri].alive {
                    return;
                }
                if !self.ff && self.remotes[ri].team == killer_team {
                    return;
                }
                self.remotes[ri].hp -= dmg;
                if self.remotes[ri].hp <= 0 {
                    self.remotes[ri].hp = 0;
                    self.remotes[ri].alive = false;
                    let victim = self.remotes[ri].name.clone();
                    let pos = self.remotes[ri].pos;
                    let yaw = self.remotes[ri].yaw;
                    let team = self.remotes[ri].team;
                    self.add_kill(&killer, killer_team, &victim);
                    play(&self.snd.death, dist_vol(self.player_eye(), pos, 0.6));
                    self.spawn_corpse(pos, yaw, team);
                    if self.carrier == Some(Ent::Remote(id)) {
                        self.carrier = None;
                        self.dropped = Some(pos);
                    }
                }
            }
            Ent::Bot(i) => {
                if i >= self.bots.len() || !self.bots[i].alive {
                    return;
                }
                if !self.ff && self.bots[i].team == killer_team {
                    return;
                }
                self.bots[i].hp -= dmg;
                self.bots[i].alert_t = 4.0;
                self.bots[i].defusing = 0.0;
                self.bots[i].planting = 0.0;
                if self.bots[i].hp <= 0 {
                    self.kill_bot(i, killer, killer_team);
                }
            }
        }
    }

    fn kill_bot(&mut self, i: usize, killer: String, killer_team: Team) {
        self.bots[i].alive = false;
        let victim = self.bots[i].name.to_string();
        let pos = self.bots[i].pos;
        let yaw = self.bots[i].yaw;
        let team = self.bots[i].team;
        self.add_kill(&killer, killer_team, &victim);
        play(&self.snd.death, dist_vol(self.player_eye(), pos, 0.6));
        self.spawn_corpse(pos, yaw, team);
        if self.carrier == Some(Ent::Bot(i)) {
            self.carrier = None;
            self.dropped = Some(pos);
        }
    }

    fn spawn_corpse(&mut self, pos: Vec3, yaw: f32, team: Team) {
        self.corpses.push(Corpse { pos, yaw, team, t: 18.0 });
        for _ in 0..14 {
            self.particles.push(Particle {
                pos: pos + vec3(0.0, 1.0, 0.0),
                vel: vec3(gen_range(-2.0, 2.0), gen_range(0.5, 3.0), gen_range(-2.0, 2.0)),
                life: gen_range(0.3, 0.6),
                max: 0.6,
                size: 0.05,
                col: Color::from_rgba(150, 12, 12, 255),
                grav: 11.0,
                drag: 0.7,
            });
        }
        if self.corpses.len() > 24 {
            self.corpses.remove(0);
        }
    }

    fn player_die(&mut self, killer: &str, killer_team: Team) {
        self.palive = false;
        self.zoom = 0.0;
        self.punch = Vec2::ZERO;
        self.buy_open = false;
        self.add_kill(killer, killer_team, "You");
        play(&self.snd.death, 0.8);
        self.show_sub("You died - LMB / Space to cycle spectate", 4.0);
        if self.carrier == Some(Ent::Me) {
            self.carrier = None;
            self.dropped = Some(self.ppos);
        }
        self.spec_i = 0;
    }

    // ---------- authority: bots + remote players sim ----------

    fn build_sim_snaps(&self) -> Vec<SimSnap> {
        let mut v = vec![SimSnap {
            ent: Ent::Me,
            eye: self.player_eye(),
            alive: self.palive,
            team: self.player_team,
        }];
        for r in &self.remotes {
            v.push(SimSnap {
                ent: Ent::Remote(r.id),
                eye: r.eye(),
                alive: r.alive,
                team: r.team,
            });
        }
        for (i, b) in self.bots.iter().enumerate() {
            v.push(SimSnap {
                ent: Ent::Bot(i),
                eye: b.eye(),
                alive: b.alive,
                team: b.team,
            });
        }
        v
    }

    fn bots_update(&mut self, dt: f32) {
        let snaps = self.build_sim_snaps();
        let mut events: Vec<DmgEvent> = Vec::new();
        let mut shot_positions: Vec<Vec3> = Vec::new();
        let bomb_planted = self.bomb.is_some() && !self.defused;
        let bomb_pos = self.bomb.as_ref().map(|b| b.pos);
        let player_eye = self.player_eye();
        let site = self.site;

        let mut planted_now: Option<Vec3> = None;
        let mut defused_now: Option<String> = None;
        let mut pickup: Option<usize> = None;

        for i in 0..self.bots.len() {
            let b = &mut self.bots[i];
            if !b.alive {
                continue;
            }
            b.alert_t = (b.alert_t - dt).max(0.0);

            b.think -= dt;
            if b.think <= 0.0 {
                b.think = 0.1 + gen_range(0.0, 0.08);
                let my_eye = b.eye();
                let facing = vec3(-b.yaw.sin(), 0.0, -b.yaw.cos());
                let mut best: Option<(Ent, f32)> = None;
                for s in &snaps {
                    if s.ent == Ent::Bot(i) || !s.alive || s.team == b.team {
                        continue;
                    }
                    let to = s.eye - my_eye;
                    let dist = to.length();
                    if dist > 55.0 {
                        continue;
                    }
                    let already = b.target == Some(s.ent);
                    if !already && b.alert_t <= 0.0 && dist > 9.0 {
                        let flat = vec3(to.x, 0.0, to.z).normalize_or_zero();
                        if facing.dot(flat) < 0.57 {
                            continue;
                        }
                    }
                    if !los_clear(my_eye, s.eye, &self.walls) {
                        continue;
                    }
                    if best.map_or(true, |(_, bd)| dist < bd) {
                        best = Some((s.ent, dist));
                    }
                }
                match (b.target, best) {
                    (None, Some((e, _))) => {
                        b.target = Some(e);
                        b.react = 0.5 + gen_range(0.0, 0.5);
                        b.can_see = true;
                        b.lost_t = 0.0;
                    }
                    (Some(_), Some((e, _))) => {
                        b.target = Some(e);
                        b.can_see = true;
                        b.lost_t = 0.0;
                    }
                    (Some(_), None) => {
                        b.can_see = false;
                        b.lost_t += 0.15;
                        if b.lost_t > 1.6 {
                            b.target = None;
                        }
                    }
                    (None, None) => {}
                }
            }

            let tsnap = b
                .target
                .and_then(|t| snaps.iter().find(|s| s.ent == t).copied())
                .filter(|s| s.alive);
            let in_combat = tsnap.is_some() && b.can_see;

            if let (true, Some(ts)) = (in_combat, tsnap) {
                let to = ts.eye - b.eye();
                let dist = to.length();
                b.yaw = angle_lerp(b.yaw, (-to.x).atan2(-to.z), 0.18);
                if b.react > 0.0 {
                    b.react -= dt;
                } else {
                    b.shot_t -= dt;
                    if b.burst > 0 && b.shot_t <= 0.0 {
                        b.shot_t = 0.11;
                        b.burst -= 1;
                        if b.burst == 0 {
                            b.burst_cd = 0.7 + gen_range(0.0, 0.7);
                        }
                        let p_hit = (0.20 - dist * 0.0033).clamp(0.04, 0.20);
                        let hit_roll = gen_range(0.0, 1.0) < p_hit;
                        let muzzle = b.eye() + to.normalize() * 0.5;
                        let jitter = if hit_roll { 0.12 } else { 1.4 };
                        let aim = ts.eye
                            + vec3(
                                gen_range(-jitter, jitter),
                                gen_range(-jitter, jitter),
                                gen_range(-jitter, jitter),
                            );
                        self.tracers.push((muzzle, aim, 0.05));
                        self.flashes.push((muzzle, 0.04));
                        shot_positions.push(muzzle);
                        self.ev_buf.push(Event::Tracer {
                            from: a3(muzzle),
                            to: a3(aim),
                        });
                        let vol = dist_vol(player_eye, muzzle, 0.8);
                        if muzzle.distance(player_eye) > 28.0 {
                            play(&self.snd.far, vol);
                        } else {
                            play(&self.snd.ak, vol * 0.8);
                        }
                        if hit_roll {
                            events.push(DmgEvent {
                                target: ts.ent,
                                dmg: gen_range(14, 26),
                                killer: b.name.to_string(),
                                killer_team: b.team,
                                src: muzzle,
                            });
                        }
                    }
                    if b.burst == 0 {
                        b.burst_cd -= dt;
                        if b.burst_cd <= 0.0 {
                            b.burst = 2 + gen_range(0, 3);
                        }
                    }
                }
                b.planting = 0.0;
                b.defusing = 0.0;
            }

            let is_carrier = self.carrier == Some(Ent::Bot(i));
            if !in_combat {
                match b.team {
                    Team::T => {
                        if is_carrier && !bomb_planted {
                            b.goal = site;
                            let flat = vec2(b.pos.x, b.pos.z);
                            if flat.distance(site) < 3.0 {
                                b.planting += dt;
                                if b.planting >= PLANT_TIME {
                                    planted_now = Some(b.pos);
                                }
                            } else {
                                b.planting = 0.0;
                            }
                        } else if !bomb_planted {
                            if self.carrier.is_none() {
                                if let Some(dp) = self.dropped {
                                    b.goal = vec2(dp.x, dp.z);
                                    if vec2(b.pos.x, b.pos.z).distance(b.goal) < 1.5 {
                                        pickup = Some(i);
                                    }
                                }
                            } else {
                                b.roam_t -= dt;
                                if b.roam_t <= 0.0 {
                                    b.roam_t = 5.0 + gen_range(0.0, 4.0);
                                    b.goal = site + vec2(gen_range(-7.0, 7.0), gen_range(-7.0, 7.0));
                                }
                            }
                        } else if let Some(bp) = bomb_pos {
                            b.roam_t -= dt;
                            if b.roam_t <= 0.0 {
                                b.roam_t = 4.0 + gen_range(0.0, 4.0);
                                b.goal = vec2(bp.x, bp.z) + vec2(gen_range(-7.0, 7.0), gen_range(-7.0, 7.0));
                            }
                        }
                    }
                    Team::Ct => {
                        if bomb_planted {
                            if let Some(bp) = bomb_pos {
                                b.goal = vec2(bp.x, bp.z);
                                if vec2(b.pos.x, b.pos.z).distance(b.goal) < 2.0 {
                                    b.defusing += dt;
                                    if b.defusing >= BOT_DEFUSE_TIME {
                                        defused_now = Some(b.name.to_string());
                                    }
                                } else {
                                    b.defusing = 0.0;
                                }
                            }
                        } else {
                            b.roam_t -= dt;
                            if b.roam_t <= 0.0 {
                                b.roam_t = 6.0 + gen_range(0.0, 5.0);
                                b.goal = b.anchor + vec2(gen_range(-4.0, 4.0), gen_range(-4.0, 4.0));
                            }
                        }
                    }
                }
            }

            let busy = b.planting > 0.0 || b.defusing > 0.0;
            let mut step = Vec3::ZERO;
            if in_combat {
                b.strafe_t -= dt;
                if b.strafe_t <= 0.0 {
                    b.strafe_t = 0.7 + gen_range(0.0, 0.8);
                    b.strafe_dir = if gen_range(0, 2) == 0 { -1.0 } else { 1.0 };
                }
                let right = vec3(b.yaw.cos(), 0.0, -b.yaw.sin());
                step = right * b.strafe_dir * BOT_SPEED * 0.45 * dt;
            } else if !busy {
                b.repath -= dt;
                let flat = vec2(b.pos.x, b.pos.z);
                let need_path = b.path_i >= b.path.len() && flat.distance(b.goal) > 1.6;
                if b.repath <= 0.0 || need_path {
                    b.repath = 1.5 + gen_range(0.0, 1.0);
                    b.path = self.nav.path(flat, b.goal);
                    b.path_i = 0;
                }
                if b.path_i < b.path.len() {
                    let wp = b.path[b.path_i];
                    let d = wp - flat;
                    if d.length() < 0.9 {
                        b.path_i += 1;
                    } else {
                        let dir = d.normalize();
                        step = vec3(dir.x, 0.0, dir.y) * BOT_SPEED * dt;
                        b.yaw = angle_lerp(b.yaw, (-dir.x).atan2(-dir.y), 0.12);
                    }
                }
            }
            b.moving = step.length_squared() > 0.0001;
            if b.moving {
                b.anim += dt * 9.0;
                b.step_t -= dt;
                if b.step_t <= 0.0 {
                    b.step_t = 0.36;
                    let v = dist_vol(player_eye, b.pos, 0.4);
                    play(if (b.anim as i32) % 2 == 0 { &self.snd.step } else { &self.snd.step2 }, v);
                }
            }
            b.vy -= GRAVITY * dt;
            let mut vy = b.vy;
            let delta = vec3(step.x, vy * dt, step.z);
            move_collide(&mut b.pos, &mut vy, 0.38, 1.75, delta, &self.walls);
            b.vy = vy;
        }

        for sp in shot_positions {
            for b in &mut self.bots {
                if b.alive && b.pos.distance(sp) < 22.0 {
                    b.alert_t = b.alert_t.max(3.0);
                }
            }
        }

        if let Some(i) = pickup {
            self.carrier = Some(Ent::Bot(i));
            self.dropped = None;
        }
        if let Some(pos) = planted_now {
            self.plant_bomb(pos);
        }
        if let Some(name) = defused_now {
            self.finish_defuse(&name);
        }

        for ev in events {
            if ev.target == Ent::Me && self.palive {
                self.add_dmg_dir(ev.src);
            }
            self.apply_damage(ev.target, ev.dmg, ev.killer, ev.killer_team);
        }
    }

    // remote players: pickup, plant, defuse (authority)
    fn remotes_update(&mut self, dt: f32) {
        let bomb_view = self.bomb.as_ref().map(|b| (b.pos, self.defused));
        let mut planted: Option<Vec3> = None;
        let mut defused_by: Option<String> = None;
        let mut picked: Option<u8> = None;
        let dropped = self.dropped;
        let carrier = self.carrier;

        for r in &mut self.remotes {
            // smooth render pos + derive a walk-cycle phase from movement
            let moved = r.render_pos.distance(r.pos);
            r.moving = moved > 0.02;
            if r.moving {
                r.anim += dt * 9.0;
            }
            r.render_pos = r.render_pos.lerp(r.pos, (dt * 14.0).min(1.0));
            r.render_yaw = angle_lerp(r.render_yaw, r.yaw, (dt * 14.0).min(1.0));
            if !r.alive {
                r.plant_p = 0.0;
                r.defuse_p = 0.0;
                continue;
            }

            let flat = vec2(r.pos.x, r.pos.z);
            let near_site = flat.distance(SITE_A) < 4.5 || flat.distance(SITE_B) < 4.5;

            if r.team == Team::T {
                if carrier.is_none() && picked.is_none() {
                    if let Some(dp) = dropped {
                        if r.pos.distance(dp) < 1.6 {
                            picked = Some(r.id);
                        }
                    }
                }
                if carrier == Some(Ent::Remote(r.id)) && bomb_view.is_none() && r.e_held && near_site {
                    r.plant_p += dt;
                    if r.plant_p >= PLANT_TIME && planted.is_none() {
                        planted = Some(r.pos);
                    }
                } else {
                    r.plant_p = 0.0;
                }
            } else if let Some((bp, defused)) = bomb_view {
                if !defused && r.e_held && r.pos.distance(bp) < 2.4 {
                    r.defuse_p += dt;
                    if r.defuse_p >= DEFUSE_TIME && defused_by.is_none() {
                        defused_by = Some(r.name.clone());
                    }
                } else {
                    r.defuse_p = 0.0;
                }
            }
        }

        if let Some(id) = picked {
            self.carrier = Some(Ent::Remote(id));
            self.dropped = None;
        }
        if let Some(pos) = planted {
            self.plant_bomb(pos);
        }
        if let Some(name) = defused_by {
            self.finish_defuse(&name);
        }
    }

    fn plant_bomb(&mut self, pos: Vec3) {
        self.bomb = Some(Bomb {
            pos,
            t: BOMB_TIME,
            beep_t: 0.0,
        });
        self.carrier = None;
        self.show_msg("The bomb has been planted", 3.0);
        play(&self.snd.planted, 0.8);
        self.ev_buf.push(Event::Planted);
    }

    fn finish_defuse(&mut self, by: &str) {
        if self.defused {
            return;
        }
        self.defused = true;
        play(&self.snd.defused, 0.9);
        self.show_sub(&format!("{by} defused the bomb"), 4.0);
        self.ev_buf.push(Event::Defused { by: by.to_string() });
        self.end_round(Team::Ct, "bomb defused");
    }

    fn bomb_update(&mut self, dt: f32) {
        if self.local_defusing() && self.is_authority() {
            self.defuse_t += dt;
            if self.defuse_t >= DEFUSE_TIME && !self.defused {
                self.finish_defuse("You");
            }
        } else if self.is_authority() {
            self.defuse_t = 0.0;
        }
        if self.is_authority() {
            if self.local_planting() {
                self.plant_t += dt;
                if self.plant_t >= PLANT_TIME && self.bomb.is_none() {
                    self.plant_bomb(self.ppos);
                    self.plant_t = 0.0;
                }
            } else {
                self.plant_t = 0.0;
            }
        }

        let mut exploded = false;
        let mut beep = 0.0f32;
        if let Some(b) = &mut self.bomb {
            if !self.defused {
                b.t -= dt;
                b.beep_t -= dt;
                if b.beep_t <= 0.0 {
                    b.beep_t = 0.12 + (b.t / BOMB_TIME).max(0.0) * 0.95;
                    beep = dist_vol(self.ppos, b.pos, 0.6).max(0.12);
                }
                if b.t <= 0.0 {
                    exploded = true;
                }
            }
        }
        if beep > 0.0 {
            play(&self.snd.beep, beep);
        }
        if exploded {
            let bp = self.bomb.as_ref().unwrap().pos;
            self.run_explosion(bp);
            self.bomb = None;
            self.end_round(Team::T, "target destroyed");
        }
    }

    fn run_explosion(&mut self, bp: Vec3) {
        self.explosion = Some((bp, 0.0));
        play(&self.snd.explosion, 1.0);
        self.shake = 0.3;
        self.ev_buf.push(Event::Explosion { pos: a3(bp) });
        let radius = 22.0;
        let d = self.ppos.distance(bp);
        if self.palive && d < radius && !self.dbg_god {
            let dmg = ((1.0 - d / radius) * 180.0) as i32;
            self.php -= dmg;
            self.vign = 1.0;
            if self.php <= 0 {
                self.php = 0;
                self.player_die("C4", Team::T);
            }
        }
        for ri in 0..self.remotes.len() {
            let d = self.remotes[ri].pos.distance(bp);
            if self.remotes[ri].alive && d < radius {
                let dmg = ((1.0 - d / radius) * 180.0) as i32;
                let id = self.remotes[ri].id;
                self.apply_damage_ff_exempt(Ent::Remote(id), dmg);
            }
        }
        for i in 0..self.bots.len() {
            let d = self.bots[i].pos.distance(bp);
            if self.bots[i].alive && d < radius {
                let dmg = ((1.0 - d / radius) * 180.0) as i32;
                self.apply_damage_ff_exempt(Ent::Bot(i), dmg);
            }
        }
    }

    fn apply_damage_ff_exempt(&mut self, target: Ent, dmg: i32) {
        let saved = self.ff;
        self.ff = true;
        self.apply_damage(target, dmg, "C4".to_string(), Team::T);
        self.ff = saved;
    }

    fn check_round_end(&mut self) {
        if self.phase != Phase::Live {
            return;
        }
        let mut ts_alive = self.bots.iter().any(|b| b.team == Team::T && b.alive)
            || (self.player_team == Team::T && self.palive);
        let mut cts_alive = self.bots.iter().any(|b| b.team == Team::Ct && b.alive)
            || (self.player_team == Team::Ct && self.palive);
        for r in &self.remotes {
            if r.alive {
                if r.team == Team::T {
                    ts_alive = true;
                } else {
                    cts_alive = true;
                }
            }
        }
        let planted = self.bomb.is_some() && !self.defused;
        if !cts_alive {
            self.end_round(Team::T, "Counter-Terrorists eliminated");
        } else if !ts_alive && !planted {
            self.end_round(Team::Ct, "Terrorists eliminated");
        } else if self.round_time <= 0.0 && !planted {
            self.end_round(Team::Ct, "time ran out");
        }
    }

    // ---------- networking glue ----------

    fn host_poll(&mut self, _dt: f32) {
        let Role::Host(_) = &self.role else { return };
        let mut joined: Vec<(u8, String, bool)> = Vec::new();
        let mut left: Vec<u8> = Vec::new();
        let mut inputs: Vec<(u8, PlayerInput)> = Vec::new();
        let mut shots: Vec<(u8, ShotMsg)> = Vec::new();
        if let Role::Host(net) = &self.role {
            while let Ok(ev) = net.events.try_recv() {
                match ev {
                    HostEvent::Ticket(t) => {
                        println!("HOST TICKET: {t}");
                        macroquad::miniquad::window::clipboard_set(&t);
                        self.ticket = Some(t);
                    }
                    HostEvent::Joined { id, name, want_t } => joined.push((id, name, want_t)),
                    HostEvent::Msg { id, msg } => match msg {
                        C2S::Input(inp) => inputs.push((id, inp)),
                        C2S::Shot(s) => shots.push((id, s)),
                        C2S::Hello { .. } => {}
                    },
                    HostEvent::Left { id } => left.push(id),
                }
            }
        }

        for (id, name, want_t) in joined {
            let team = Team::from_t(want_t);
            let slot = 1 + self.remotes.iter().filter(|r| r.team == team).count();
            let (pos, yaw) = self.team_spawn(team, slot);
            let mut rp = RPlayer {
                id,
                name: name.clone(),
                team,
                pos,
                yaw,
                hp: 100,
                alive: true,
                e_held: false,
                weapon: 0,
                plant_p: 0.0,
                defuse_p: 0.0,
                spawn_seq: 1,
                render_pos: pos,
                render_yaw: yaw,
                anim: 0.0,
                moving: false,
                kills: 0,
                deaths: 0,
            };
            rp.render_pos = pos;
            self.remotes.push(rp);
            if let Role::Host(net) = &self.role {
                net.send_to(id, S2C::Welcome { id, team_t: want_t, ff: self.ff });
                net.send_to(id, S2C::Spawn { pos: a3(pos), yaw, seq: 1 });
            }
            self.show_sub(&format!("{name} joined"), 3.0);
            println!("player joined: {name} (id {id})");
        }

        for id in left {
            if let Some(i) = self.remotes.iter().position(|r| r.id == id) {
                let name = self.remotes[i].name.clone();
                if self.carrier == Some(Ent::Remote(id)) {
                    self.carrier = None;
                    self.dropped = Some(self.remotes[i].pos);
                }
                self.remotes.remove(i);
                self.show_sub(&format!("{name} left"), 3.0);
                println!("player left: {name} (id {id})");
            }
        }

        for (id, inp) in inputs {
            if let Some(r) = self.remotes.iter_mut().find(|r| r.id == id) {
                if inp.alive_seq == r.spawn_seq && r.alive {
                    let p = v3(inp.pos);
                    // sanity clamp inside arena
                    r.pos = vec3(p.x.clamp(-40.0, 40.0), p.y.clamp(0.0, 10.0), p.z.clamp(-30.0, 30.0));
                    r.yaw = inp.yaw;
                    r.e_held = inp.e_held;
                    r.weapon = inp.weapon;
                }
            }
        }

        for (id, s) in shots {
            let Some(r) = self.remotes.iter().find(|r| r.id == id) else {
                continue;
            };
            if !r.alive || self.phase != Phase::Live {
                continue;
            }
            let shooter_name = r.name.clone();
            let shooter_team = r.team;
            let from = v3(s.from);
            let to = v3(s.to);
            self.tracers.push((from, to, 0.05));
            self.ev_buf.push(Event::Tracer { from: s.from, to: s.to });
            let vol = dist_vol(self.player_eye(), from, 0.8);
            if s.pistol {
                play(&self.snd.pistol, vol * 0.9);
            } else {
                play(&self.snd.ak, vol * 0.9);
            }
            for b in &mut self.bots {
                if b.alive && b.pos.distance(from) < 28.0 {
                    b.alert_t = b.alert_t.max(3.0);
                }
            }
            if let Some(claim) = s.hit {
                match claim {
                    HitClaim::Bot { idx, dmg } => {
                        self.apply_damage(Ent::Bot(idx as usize), dmg, shooter_name, shooter_team)
                    }
                    HitClaim::Player { id: tid, dmg } => {
                        let target = if tid == 0 { Ent::Me } else { Ent::Remote(tid) };
                        self.apply_damage(target, dmg, shooter_name, shooter_team)
                    }
                }
            }
        }
    }

    fn host_broadcast(&mut self, dt: f32) {
        let Role::Host(_) = &self.role else {
            self.ev_buf.clear();
            return;
        };
        self.snap_timer -= dt;
        if self.snap_timer > 0.0 {
            return;
        }
        self.snap_timer = NET_RATE;

        let mut players = vec![PlayerNet {
            id: 0,
            name: self.my_name.clone(),
            team_t: self.player_team.is_t(),
            pos: a3(self.ppos),
            yaw: self.yaw,
            alive: self.palive,
            hp: self.php,
            carrier: self.carrier == Some(Ent::Me),
            progress: (self.defuse_t / DEFUSE_TIME).max(self.plant_t / PLANT_TIME),
        }];
        for r in &self.remotes {
            players.push(PlayerNet {
                id: r.id,
                name: r.name.clone(),
                team_t: r.team.is_t(),
                pos: a3(r.pos),
                yaw: r.yaw,
                alive: r.alive,
                hp: r.hp,
                carrier: self.carrier == Some(Ent::Remote(r.id)),
                progress: (r.plant_p / PLANT_TIME).max(r.defuse_p / DEFUSE_TIME),
            });
        }
        let bots = self
            .bots
            .iter()
            .enumerate()
            .map(|(i, b)| BotNet {
                name: b.name.to_string(),
                team_t: b.team.is_t(),
                pos: a3(b.pos),
                yaw: b.yaw,
                alive: b.alive,
                carrier: self.carrier == Some(Ent::Bot(i)),
            })
            .collect();
        let snap = Snapshot {
            players,
            bots,
            bomb: self.bomb.as_ref().map(|b| (a3(b.pos), b.t, self.defused)),
            dropped: self.dropped.map(a3),
            phase: match self.phase {
                Phase::Freeze => 0,
                Phase::Live => 1,
                _ => 2,
            },
            round_time: self.round_time,
            score_ct: self.score_ct,
            score_t: self.score_t,
            round: self.round,
            events: std::mem::take(&mut self.ev_buf),
        };
        if let Role::Host(net) = &self.role {
            net.broadcast(S2C::Snap(snap));
        }
    }

    fn client_poll(&mut self) {
        let Role::Client(_) = &self.role else { return };
        let mut msgs: Vec<S2C> = Vec::new();
        let mut disconnect: Option<String> = None;
        if let Role::Client(net) = &self.role {
            while let Ok(ev) = net.events.try_recv() {
                match ev {
                    ClientEvent::Connected => {}
                    ClientEvent::Msg(m) => msgs.push(m),
                    ClientEvent::Disconnected(why) => disconnect = Some(why),
                }
            }
        }
        if let Some(why) = disconnect {
            self.cstate.disconnect = Some(why);
        }
        for m in msgs {
            match m {
                S2C::Welcome { id, team_t, ff } => {
                    self.cstate.my_id = id;
                    self.cstate.welcomed = true;
                    self.player_team = Team::from_t(team_t);
                    self.ff = ff;
                    if self.phase == Phase::Menu {
                        self.phase = Phase::Live; // actual phase comes from snapshots
                    }
                }
                S2C::Spawn { pos, yaw, seq } => {
                    self.ppos = v3(pos);
                    self.yaw = yaw;
                    self.pitch = 0.0;
                    self.pvel = Vec3::ZERO;
                    self.palive = true;
                    self.php = 100;
                    self.cstate.spawn_seq = seq;
                    self.cstate.prev_alive = true;
                    self.cstate.prev_hp = 100;
                    self.refill_loadout();
                    self.cur = if WPNS[self.loadout[0]].slot == 0 { 0 } else { 1 };
                    self.defuse_t = 0.0;
                    self.plant_t = 0.0;
                    self.punch = Vec2::ZERO;
                    self.zoom = 0.0;
                    self.vign = 0.0;
                }
                S2C::Snap(snap) => self.client_apply_snap(snap),
            }
        }
    }

    fn client_apply_snap(&mut self, snap: Snapshot) {
        let my_eye = self.player_eye();
        for ev in &snap.events {
            match ev {
                Event::Kill { killer, killer_ct, victim } => {
                    let me = self
                        .cstate
                        .snap
                        .as_ref()
                        .and_then(|s| s.players.iter().find(|p| p.id == self.cstate.my_id))
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let victim_disp = if *victim == me { "You".to_string() } else { victim.clone() };
                    let killer_disp = if *killer == me { "You".to_string() } else { killer.clone() };
                    let team = if *killer_ct { Team::Ct } else { Team::T };
                    let c = if *killer_ct {
                        Color::from_rgba(120, 180, 255, 255)
                    } else {
                        Color::from_rgba(255, 180, 110, 255)
                    };
                    self.killfeed.push((format!("{killer_disp} > {victim_disp}"), c, 5.0));
                    if self.killfeed.len() > 6 {
                        self.killfeed.remove(0);
                    }
                    let _ = team;
                }
                Event::Tracer { from, to } => {
                    let f = v3(*from);
                    if f.distance(my_eye) > 1.5 {
                        self.tracers.push((f, v3(*to), 0.05));
                        let vol = dist_vol(my_eye, f, 0.8);
                        if f.distance(my_eye) > 28.0 {
                            play(&self.snd.far, vol);
                        } else {
                            play(&self.snd.ak, vol * 0.8);
                        }
                    }
                }
                Event::Planted => {
                    self.show_msg("The bomb has been planted", 3.0);
                    play(&self.snd.planted, 0.8);
                }
                Event::Defused { by } => {
                    play(&self.snd.defused, 0.9);
                    self.show_sub(&format!("{by} defused the bomb"), 4.0);
                }
                Event::Explosion { pos } => {
                    let p = v3(*pos);
                    self.explosion = Some((p, 0.0));
                    play(&self.snd.explosion, 1.0);
                    self.shake = 0.3;
                }
                Event::RoundEnd { ct_won, reason } => {
                    let who = if *ct_won {
                        "Counter-Terrorists win"
                    } else {
                        "Terrorists win"
                    };
                    self.show_msg(&format!("{who} - {reason}"), 4.0);
                }
                Event::RoundMsg { round } => {
                    self.show_msg(&format!("Round {round}"), 2.5);
                }
            }
        }

        // own authoritative state
        if let Some(me) = snap.players.iter().find(|p| p.id == self.cstate.my_id) {
            if me.hp < self.cstate.prev_hp && me.alive {
                self.vign = (self.vign + 0.45).min(1.0);
                self.shake = 0.02;
            }
            self.php = me.hp;
            if self.cstate.prev_alive && !me.alive {
                self.palive = false;
                self.zoom = 0.0;
                self.punch = Vec2::ZERO;
                play(&self.snd.death, 0.8);
                self.show_sub("You died - LMB / Space to cycle spectate", 4.0);
            }
            self.cstate.prev_hp = me.hp;
            self.cstate.prev_alive = me.alive;
            if me.carrier && self.bomb_view().is_none() && self.sub_t <= 0.0 && self.msg_t <= 0.0 {
                // gentle reminder handled by HUD C4 indicator instead
            }
        }

        // remote entity footsteps, derived from movement
        if let Some(prev) = &self.cstate.snap {
            for p in &snap.players {
                if p.id == self.cstate.my_id || !p.alive {
                    continue;
                }
                if let Some(pp) = prev.players.iter().find(|q| q.id == p.id) {
                    let moved = v3(p.pos).distance(v3(pp.pos));
                    let e = self.cstate.step_acc.entry(p.id).or_insert((v3(p.pos), 0.0));
                    e.1 += moved;
                    if e.1 > 2.4 {
                        e.1 = 0.0;
                        play(&self.snd.step, dist_vol(my_eye, v3(p.pos), 0.4));
                    }
                }
            }
        }

        self.cstate.snap = Some(snap);
    }

    fn client_send_input(&mut self, dt: f32) {
        let Role::Client(_) = &self.role else { return };
        self.input_timer -= dt;
        if self.input_timer > 0.0 {
            return;
        }
        self.input_timer = NET_RATE;
        let inp = PlayerInput {
            pos: a3(self.ppos),
            yaw: self.yaw,
            pitch: self.pitch,
            weapon: self.cur as u8,
            e_held: is_key_down(KeyCode::E) && self.palive,
            alive_seq: self.cstate.spawn_seq,
        };
        if let Role::Client(net) = &self.role {
            net.send(C2S::Input(inp));
        }
    }

    // ---------- drawing ----------

    fn draw_obox(&self, center: Vec3, yaw: f32, half: Vec3, c: Color) {
        let (s, co) = yaw.sin_cos();
        let r = vec3(co, 0.0, -s) * half.x;
        let u = vec3(0.0, 1.0, 0.0) * half.y;
        let f = vec3(-s, 0.0, -co) * half.z;
        let mut verts = Vec::with_capacity(24);
        let mut inds = Vec::with_capacity(36);
        // light each face by its true world normal so models sit in the scene light
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            (u, r, f),
            (-u, r, f),
            (f, r, u),
            (-f, r, u),
            (r, f, u),
            (-r, f, u),
        ];
        for (n, a, b) in faces {
            let p = [
                center + n + a + b,
                center + n - a + b,
                center + n - a - b,
                center + n + a - b,
            ];
            let col = if c.a < 0.99 {
                c
            } else {
                apply_light(c, n.normalize_or_zero(), 1.0)
            };
            push_quad(&mut verts, &mut inds, p, col);
        }
        draw_mesh(&Mesh { vertices: verts, indices: inds, texture: None });
    }

    // single camera/normal-aligned quad (particles, decals)
    fn quad3(&self, c: Vec3, ax: Vec3, ay: Vec3, col: Color) {
        let p = [c - ax - ay, c + ax - ay, c + ax + ay, c - ax + ay];
        let mut v = Vec::with_capacity(4);
        let mut i = Vec::with_capacity(6);
        push_quad(&mut v, &mut i, p, col);
        draw_mesh(&Mesh { vertices: v, indices: i, texture: None });
    }

    fn draw_character(&self, e: &DrawEnt) {
        let (gear, accent) = if e.team == Team::T {
            (Color::from_rgba(150, 120, 86, 255), Color::from_rgba(196, 142, 58, 255))
        } else {
            (Color::from_rgba(70, 86, 120, 255), Color::from_rgba(72, 120, 200, 255))
        };
        let skin = Color::from_rgba(208, 170, 132, 255);
        let dark = Color::from_rgba(38, 36, 34, 255);
        let (s, co) = e.yaw.sin_cos();
        let fwd = vec3(-s, 0.0, -co);
        let right = vec3(co, 0.0, -s);

        // walk cycle: legs swing, body bobs
        let sw = if e.moving { e.phase.sin() * 0.22 } else { 0.0 };
        let bob = if e.moving { (e.phase * 2.0).sin().abs() * 0.04 } else { 0.0 };
        let base = e.pos + vec3(0.0, bob, 0.0);

        // soft contact shadow
        self.draw_obox(e.pos + vec3(0.0, 0.02, 0.0), e.yaw, vec3(0.42, 0.001, 0.42), Color::new(0.0, 0.0, 0.0, 0.32));

        // legs
        for sgn in [-1.0f32, 1.0] {
            let swing = sw * sgn;
            self.draw_obox(
                base + vec3(0.0, 0.42, 0.0) + right * (0.12 * sgn) + fwd * swing,
                e.yaw,
                vec3(0.11, 0.42, 0.14),
                gear,
            );
            // boot
            self.draw_obox(
                base + vec3(0.0, 0.06, 0.0) + right * (0.12 * sgn) + fwd * (swing + 0.04),
                e.yaw,
                vec3(0.12, 0.07, 0.18),
                dark,
            );
        }
        // pelvis
        self.draw_obox(base + vec3(0.0, 0.86, 0.0), e.yaw, vec3(0.24, 0.12, 0.16), gear);
        // torso + tactical vest
        self.draw_obox(base + vec3(0.0, 1.18, 0.0), e.yaw, vec3(0.27, 0.30, 0.17), gear);
        self.draw_obox(base + vec3(0.0, 1.18, 0.0) + fwd * 0.04, e.yaw, vec3(0.25, 0.22, 0.16), shade(accent, 0.9));
        // shoulders
        self.draw_obox(base + vec3(0.0, 1.42, 0.0), e.yaw, vec3(0.33, 0.08, 0.16), shade(gear, 1.05));
        // neck + head
        self.draw_obox(base + vec3(0.0, 1.55, 0.0), e.yaw, vec3(0.08, 0.06, 0.08), skin);
        self.draw_obox(base + vec3(0.0, 1.68, 0.0), e.yaw, vec3(0.13, 0.14, 0.14), skin);
        // headgear: CT helmet, T balaclava+cap
        if e.team == Team::Ct {
            self.draw_obox(base + vec3(0.0, 1.79, 0.0), e.yaw, vec3(0.145, 0.06, 0.15), dark);
        } else {
            self.draw_obox(base + vec3(0.0, 1.70, 0.0), e.yaw, vec3(0.135, 0.10, 0.145), dark);
            self.draw_obox(base + vec3(0.0, 1.80, 0.0), e.yaw, vec3(0.14, 0.04, 0.15), shade(accent, 0.7));
        }
        // back arm
        self.draw_obox(base + vec3(0.0, 1.18, 0.0) - right * 0.30 - fwd * 0.02, e.yaw, vec3(0.07, 0.24, 0.09), gear);

        // weapon held forward + front arm bracing it
        let gun = base + vec3(0.0, 1.24, 0.0) + fwd * 0.34 + right * 0.10;
        self.draw_obox(gun, e.yaw, vec3(0.05, 0.06, 0.32), dark);
        self.draw_obox(gun + fwd * 0.18 - vec3(0.0, 0.05, 0.0), e.yaw, vec3(0.035, 0.05, 0.12), shade(dark, 1.3));
        self.draw_obox(base + vec3(0.0, 1.24, 0.0) + fwd * 0.22 + right * 0.16, e.yaw, vec3(0.06, 0.07, 0.10), skin);

        if e.carrier {
            self.draw_obox(base + vec3(0.0, 1.12, 0.0) - fwd * 0.26, e.yaw, vec3(0.17, 0.20, 0.08), Color::from_rgba(120, 40, 32, 255));
            self.draw_obox(base + vec3(0.0, 1.18, 0.0) - fwd * 0.31, e.yaw, vec3(0.05, 0.05, 0.04), Color::from_rgba(40, 200, 60, 255));
        }
    }

    fn draw_corpse(&self, cp: &Corpse) {
        let gear = if cp.team == Team::T {
            Color::from_rgba(120, 96, 70, 255)
        } else {
            Color::from_rgba(58, 72, 100, 255)
        };
        let skin = Color::from_rgba(180, 146, 112, 255);
        let (s, co) = cp.yaw.sin_cos();
        let fwd = vec3(-s, 0.0, -co);
        let fade = (cp.t / 4.0).clamp(0.0, 1.0);
        let a = (220.0 * fade) as u8;
        let g = Color::new(gear.r, gear.g, gear.b, a as f32 / 255.0);
        let sk = Color::new(skin.r, skin.g, skin.b, a as f32 / 255.0);
        // a flattened body sprawled along the facing direction
        self.draw_obox(cp.pos + vec3(0.0, 0.12, 0.0) - fwd * 0.1, cp.yaw, vec3(0.28, 0.12, 0.22), g);
        self.draw_obox(cp.pos + vec3(0.0, 0.12, 0.0) + fwd * 0.45, cp.yaw, vec3(0.22, 0.10, 0.16), g);
        self.draw_obox(cp.pos + vec3(0.0, 0.12, 0.0) + fwd * 0.72, cp.yaw, vec3(0.13, 0.10, 0.13), sk);
    }

    fn collect_draw_ents(&self) -> Vec<DrawEnt> {
        let mut out = Vec::new();
        if self.is_authority() {
            for (i, b) in self.bots.iter().enumerate() {
                if b.alive {
                    out.push(DrawEnt {
                        pos: b.pos,
                        yaw: b.yaw,
                        team: b.team,
                        carrier: self.carrier == Some(Ent::Bot(i)),
                        name: String::new(),
                        hp: b.hp,
                        phase: b.anim,
                        moving: b.moving,
                    });
                }
            }
            for r in &self.remotes {
                if r.alive {
                    out.push(DrawEnt {
                        pos: r.render_pos,
                        yaw: r.render_yaw,
                        team: r.team,
                        carrier: self.carrier == Some(Ent::Remote(r.id)),
                        name: r.name.clone(),
                        hp: r.hp,
                        phase: r.anim,
                        moving: r.moving,
                    });
                }
            }
        } else if let Some(snap) = &self.cstate.snap {
            let t = get_time() as f32 * 9.0;
            for b in &snap.bots {
                if b.alive {
                    out.push(DrawEnt {
                        pos: v3(b.pos),
                        yaw: b.yaw,
                        team: Team::from_t(b.team_t),
                        carrier: b.carrier,
                        name: String::new(),
                        hp: 100,
                        phase: t + b.pos[0],
                        moving: true,
                    });
                }
            }
            for p in &snap.players {
                if p.alive && p.id != self.cstate.my_id {
                    out.push(DrawEnt {
                        pos: v3(p.pos),
                        yaw: p.yaw,
                        team: Team::from_t(p.team_t),
                        carrier: p.carrier,
                        name: p.name.clone(),
                        hp: p.hp,
                        phase: t + p.pos[0],
                        moving: true,
                    });
                }
            }
        }
        out
    }

    fn spec_candidates(&self) -> Vec<(Vec3, f32, String)> {
        let mut v = Vec::new();
        if self.is_authority() {
            for b in &self.bots {
                if b.alive && b.team == self.player_team {
                    v.push((b.eye(), b.yaw, b.name.to_string()));
                }
            }
            for r in &self.remotes {
                if r.alive && r.team == self.player_team {
                    v.push((r.eye(), r.render_yaw, r.name.clone()));
                }
            }
        } else if let Some(snap) = &self.cstate.snap {
            for p in &snap.players {
                if p.alive && p.id != self.cstate.my_id && Team::from_t(p.team_t) == self.player_team {
                    v.push((v3(p.pos) + vec3(0.0, EYE, 0.0), p.yaw, p.name.clone()));
                }
            }
            for b in &snap.bots {
                if b.alive && Team::from_t(b.team_t) == self.player_team {
                    v.push((v3(b.pos) + vec3(0.0, 1.5, 0.0), b.yaw, b.name.clone()));
                }
            }
        }
        v
    }

    fn draw_world(&self, t_now: f64, cam: &Camera3D) {
        for m in &self.meshes {
            draw_mesh(m);
        }
        let ents = self.collect_draw_ents();
        for e in &ents {
            // skip whoever the camera is inside (spectated player/bot)
            if (e.pos + vec3(0.0, 1.5, 0.0)).distance(cam.position) < 0.8 {
                continue;
            }
            self.draw_character(e);
        }
        for cp in &self.corpses {
            self.draw_corpse(cp);
        }
        // decals: bullet scorches and blood, offset off their surface
        for d in &self.decals {
            let n = d.n;
            let t1 = if n.y.abs() > 0.9 {
                vec3(1.0, 0.0, 0.0)
            } else {
                vec3(0.0, 1.0, 0.0).cross(n).normalize_or_zero()
            };
            let t2 = n.cross(t1).normalize_or_zero();
            let fade = (d.life / 3.0).clamp(0.0, 1.0);
            let col = Color::new(d.col.r, d.col.g, d.col.b, d.col.a * fade);
            self.quad3(d.pos, t1 * d.size, t2 * d.size, col);
        }
        // particles, billboarded to the camera
        let cdir = (cam.target - cam.position).normalize_or_zero();
        let cright = cdir.cross(cam.up).normalize_or_zero();
        let cup = cright.cross(cdir).normalize_or_zero();
        for p in &self.particles {
            let f = (p.life / p.max).clamp(0.0, 1.0);
            let col = Color::new(p.col.r, p.col.g, p.col.b, p.col.a * f);
            let s = p.size * (0.55 + 0.45 * f);
            self.quad3(p.pos, cright * s, cup * s, col);
        }
        let dropped_view = if self.is_authority() {
            self.dropped
        } else {
            self.cstate.snap.as_ref().and_then(|s| s.dropped).map(v3)
        };
        if let Some(dp) = dropped_view {
            self.draw_obox(
                dp + vec3(0.0, 0.12, 0.0),
                0.0,
                vec3(0.2, 0.12, 0.12),
                Color::from_rgba(120, 35, 30, 255),
            );
        }
        if let Some((bp, t, defused)) = self.bomb_view() {
            self.draw_obox(
                bp + vec3(0.0, 0.12, 0.0),
                0.6,
                vec3(0.22, 0.12, 0.14),
                Color::from_rgba(110, 32, 28, 255),
            );
            let blink = ((t_now * (2.0 + (1.0 - (t / BOMB_TIME) as f64) * 10.0)).sin() > 0.0) as i32;
            if blink == 1 && !defused {
                draw_sphere(bp + vec3(0.12, 0.28, 0.0), 0.05, None, RED);
            }
        }
        for (a, b, life) in &self.tracers {
            let f = (life / 0.05).clamp(0.0, 1.0);
            let mid = a.lerp(*b, 0.5);
            // bright hot core plus a soft amber sheath billboarded outward
            self.quad3(mid, (*b - *a) * 0.5, cright * 0.018 * f, Color::new(1.0, 0.7, 0.25, 0.5 * f));
            draw_line_3d(*a, *b, Color::new(1.0, 0.92, 0.6, 0.95 * f));
        }
        for (p, life) in &self.flashes {
            let f = (life / 0.04).clamp(0.0, 1.0);
            self.quad3(*p, cright * 0.16 * f, cup * 0.16 * f, Color::new(1.0, 0.85, 0.4, 0.95 * f));
            self.quad3(*p, cright * 0.07, cup * 0.07, Color::new(1.0, 0.98, 0.85, f));
        }
        if let Some((p, age)) = self.explosion {
            let r = 1.0 + age * 26.0;
            let a = (1.0 - age / 0.7).max(0.0);
            draw_sphere(p + vec3(0.0, 1.0, 0.0), r, None, Color::new(1.0, 0.55, 0.12, a * 0.7));
            draw_sphere(p + vec3(0.0, 1.0, 0.0), r * 0.6, None, Color::new(1.0, 0.9, 0.5, a));
            draw_sphere(p + vec3(0.0, 1.0, 0.0), r * 1.25, None, Color::new(0.2, 0.2, 0.2, a * 0.25));
        }
        if self.dbg_paths && self.is_authority() {
            for b in &self.bots {
                if !b.alive {
                    continue;
                }
                let col = if b.team == Team::T {
                    Color::new(1.0, 0.6, 0.2, 0.8)
                } else {
                    Color::new(0.3, 0.6, 1.0, 0.8)
                };
                let mut prev = b.pos + vec3(0.0, 0.4, 0.0);
                for wp in b.path.iter().skip(b.path_i) {
                    let p = vec3(wp.x, 0.4, wp.y);
                    draw_line_3d(prev, p, col);
                    prev = p;
                }
                if let Some(t) = b.target {
                    if b.can_see {
                        let te = match t {
                            Ent::Me => self.player_eye(),
                            Ent::Remote(id) => self
                                .remotes
                                .iter()
                                .find(|r| r.id == id)
                                .map(|r| r.eye())
                                .unwrap_or(b.eye()),
                            Ent::Bot(j) => self.bots.get(j).map(|x| x.eye()).unwrap_or(b.eye()),
                        };
                        draw_line_3d(b.eye(), te, Color::new(1.0, 0.1, 0.1, 0.9));
                    }
                }
            }
        }
        let _ = cam;
    }

    fn draw_viewmodel(&self) {
        if !self.palive || self.phase == Phase::Menu {
            return;
        }
        // when fully scoped, hide the gun and let the scope overlay take over
        if self.zoom > 0.85 {
            return;
        }
        let wi = self.loadout[self.cur];
        let def = &WPNS[wi];
        let eye = self.player_eye();
        let dir = self.look_dir();
        let right = self.flat_right();
        let up = right.cross(dir).normalize();
        let bob = self.bob.sin() * 0.012 * if def.sniper { 0.4 } else { 1.0 };
        let sway = self.punch.x * 0.06;
        let reload_dip = if self.wpn[self.cur].reload_t > 0.0 {
            (1.0 - (self.wpn[self.cur].reload_t / def.reload - 0.5).abs() * 2.0).max(0.0) * 0.22
        } else {
            0.0
        };
        let switch_dip = if self.switch_t > 0.0 { self.switch_t * 0.5 } else { 0.0 };
        let kick = if self.muzzle_t > 0.0 { 0.035 + def.recoil } else { 0.0 } + self.punch.y * 0.25;
        // ADS pulls the weapon toward centre
        let ads = self.zoom;
        let rightoff = 0.24 * (1.0 - ads * 0.85) + sway;
        let base = eye + dir * (0.55 - kick) + right * rightoff
            - up * (0.26 + bob + reload_dip + switch_dip - ads * 0.05);
        let gyaw = self.yaw + self.punch.x;
        let dark = Color::from_rgba(42, 40, 38, 255);
        let metal = Color::from_rgba(70, 70, 76, 255);
        let wood = Color::from_rgba(108, 74, 44, 255);
        let mut muzzle_fwd = 0.4;
        match def.slot {
            2 => {
                // knife
                self.draw_obox(base + dir * 0.18 + up * 0.02, gyaw, vec3(0.012, 0.05, 0.16), metal);
                self.draw_obox(base + dir * 0.0 - up * 0.04, gyaw, vec3(0.02, 0.03, 0.06), dark);
            }
            1 => {
                // pistol
                self.draw_obox(base + dir * 0.12, gyaw, vec3(0.03, 0.05, 0.16), dark);
                self.draw_obox(base - dir * 0.02 - up * 0.08, gyaw, vec3(0.028, 0.09, 0.04), dark);
                muzzle_fwd = 0.3;
            }
            _ if def.sniper => {
                self.draw_obox(base + dir * 0.30, gyaw, vec3(0.03, 0.04, 0.46), dark);
                self.draw_obox(base + dir * 0.10 + up * 0.07, gyaw, vec3(0.02, 0.04, 0.14), metal); // scope
                self.draw_obox(base + dir * 0.02 - up * 0.08, gyaw, vec3(0.03, 0.09, 0.05), dark);
                self.draw_obox(base - dir * 0.22, gyaw, vec3(0.035, 0.06, 0.14), wood);
                muzzle_fwd = 0.78;
            }
            _ => {
                // rifle / smg
                let stock = if wi == W_AK { wood } else { dark };
                self.draw_obox(base + dir * 0.27, gyaw, vec3(0.035, 0.05, 0.38), dark);
                self.draw_obox(base + dir * 0.40 + up * 0.05, gyaw, vec3(0.015, 0.03, 0.07), metal);
                self.draw_obox(base + dir * 0.02 - up * 0.08, gyaw, vec3(0.03, 0.10, 0.05), stock);
                self.draw_obox(base - dir * 0.20, gyaw, vec3(0.035, 0.06, 0.14), stock);
                muzzle_fwd = if wi == W_MP5 { 0.5 } else { 0.66 };
            }
        }
        if self.muzzle_t > 0.0 {
            let mpos = base + dir * muzzle_fwd;
            let r = 0.06 + def.recoil;
            draw_sphere(mpos, r, None, Color::new(1.0, 0.86, 0.42, 0.95));
            draw_sphere(mpos + dir * 0.04, r * 0.5, None, Color::new(1.0, 1.0, 0.9, 0.95));
        }
    }

    // dark scope overlay drawn in 2D when fully zoomed
    fn draw_scope(&self) {
        if self.zoom <= 0.85 || !self.palive {
            return;
        }
        let (sw, sh) = (screen_width(), screen_height());
        let (cx, cy) = (sw / 2.0, sh / 2.0);
        let rad = sh * 0.46;
        // black-out corners around a circular lens
        draw_rectangle(0.0, 0.0, sw, cy - rad, BLACK);
        draw_rectangle(0.0, cy + rad, sw, cy - rad, BLACK);
        draw_rectangle(0.0, 0.0, cx - rad, sh, BLACK);
        draw_rectangle(cx + rad, 0.0, cx - rad, sh, BLACK);
        draw_circle_lines(cx, cy, rad, 2.0, Color::new(0.0, 0.0, 0.0, 0.9));
        // crosshair lines
        let c = Color::new(0.0, 0.0, 0.0, 0.9);
        draw_line(cx, cy - rad, cx, cy + rad, 1.0, c);
        draw_line(cx - rad, cy, cx + rad, cy, 1.0, c);
        draw_line(cx - 14.0, cy, cx + 14.0, cy, 1.0, c);
    }

    fn project(&self, cam: &Camera3D, p: Vec3) -> Option<Vec2> {
        let clip = cam.matrix() * vec4(p.x, p.y, p.z, 1.0);
        if clip.w <= 0.01 {
            return None;
        }
        let ndc = vec2(clip.x / clip.w, clip.y / clip.w);
        Some(vec2(
            (ndc.x * 0.5 + 0.5) * screen_width(),
            (1.0 - (ndc.y * 0.5 + 0.5)) * screen_height(),
        ))
    }

    fn hud_numbers(&self) -> (i32, i32, f32, bool, u8) {
        if self.is_authority() {
            let planted = self.bomb.is_some() && !self.defused;
            let timer = if let Some(b) = &self.bomb {
                if self.defused {
                    0.0
                } else {
                    b.t
                }
            } else {
                self.round_time
            };
            let phase = match self.phase {
                Phase::Freeze => 0,
                Phase::Live => 1,
                _ => 2,
            };
            (self.score_ct, self.score_t, timer, planted, phase)
        } else if let Some(s) = &self.cstate.snap {
            let planted = s.bomb.map_or(false, |(_, _, d)| !d);
            let timer = s.bomb.map_or(s.round_time, |(_, t, d)| if d { 0.0 } else { t });
            (s.score_ct, s.score_t, timer, planted, s.phase)
        } else {
            (0, 0, 0.0, false, 0)
        }
    }

    fn draw_hud(&mut self, cam: &Camera3D) {
        let sw = screen_width();
        let sh = screen_height();
        let cx = sw / 2.0;
        let cy = sh / 2.0;

        for (site, label) in [(SITE_A, "A"), (SITE_B, "B")] {
            if let Some(s) = self.project(cam, vec3(site.x, 3.5, site.y)) {
                let m = measure_text(label, None, 34, 1.0);
                draw_text(label, s.x - m.width / 2.0, s.y, 34.0, Color::new(1.0, 0.8, 0.4, 0.75));
            }
        }
        if let Some((bp, _, defused)) = self.bomb_view() {
            if !defused {
                if let Some(s) = self.project(cam, bp + vec3(0.0, 0.8, 0.0)) {
                    let m = measure_text("C4", None, 22, 1.0);
                    draw_text("C4", s.x - m.width / 2.0, s.y, 22.0, Color::new(1.0, 0.3, 0.25, 0.9));
                }
            }
        }

        // overhead tags: names for humans, plus health bars for teammates
        for e in self.collect_draw_ents() {
            if (e.pos + vec3(0.0, 1.5, 0.0)).distance(cam.position) < 0.8 {
                continue;
            }
            let friend = e.team == self.player_team;
            let col = if e.team == Team::T {
                Color::from_rgba(255, 180, 110, 230)
            } else {
                Color::from_rgba(140, 190, 255, 230)
            };
            if !e.name.is_empty() {
                if let Some(s) = self.project(cam, e.pos + vec3(0.0, 2.18, 0.0)) {
                    let m = measure_text(&e.name, None, 16, 1.0);
                    draw_text(&e.name, s.x - m.width / 2.0, s.y, 16.0, col);
                }
            }
            // teammate health bar (intel you'd have on your own squad)
            if friend {
                if let Some(s) = self.project(cam, e.pos + vec3(0.0, 2.02, 0.0)) {
                    let bw = 40.0;
                    let hpf = (e.hp as f32 / 100.0).clamp(0.0, 1.0);
                    draw_rectangle(s.x - bw / 2.0, s.y, bw, 4.0, Color::new(0.0, 0.0, 0.0, 0.6));
                    draw_rectangle(s.x - bw / 2.0, s.y, bw * hpf, 4.0, Color::from_rgba(90, 210, 110, 230));
                }
            }
        }

        if self.dbg_esp {
            for e in self.collect_draw_ents() {
                let col = if e.team == Team::T {
                    Color::from_rgba(255, 160, 70, 255)
                } else {
                    Color::from_rgba(110, 170, 255, 255)
                };
                if let (Some(top), Some(bottom)) = (
                    self.project(cam, e.pos + vec3(0.0, 1.85, 0.0)),
                    self.project(cam, e.pos),
                ) {
                    let h = (bottom.y - top.y).abs().max(4.0);
                    let w = h * 0.45;
                    draw_rectangle_lines(top.x - w / 2.0, top.y, w, h, 1.5, col);
                }
            }
        }

        self.draw_dmg_numbers(cam);
        self.draw_scope();

        if self.palive && self.zoom < 0.85 {
            let ch = Color::from_rgba(70, 255, 120, 235);
            let gap = 4.0 + self.spread_add * 250.0;
            let len = 9.0;
            draw_line(cx - gap - len, cy, cx - gap, cy, 2.0, ch);
            draw_line(cx + gap, cy, cx + gap + len, cy, 2.0, ch);
            draw_line(cx, cy - gap - len, cx, cy - gap, 2.0, ch);
            draw_line(cx, cy + gap, cx, cy + gap + len, 2.0, ch);
            draw_circle(cx, cy, 1.0, ch);
        }

        if self.hitm > 0.0 {
            let a = (self.hitm / 0.12).clamp(0.0, 1.0);
            let c = if self.hitk {
                Color::new(1.0, 0.3, 0.25, a)
            } else {
                Color::new(1.0, 1.0, 1.0, a)
            };
            let sz = if self.hitk { 16.0 } else { 13.0 };
            for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                draw_line(cx + dx * 6.0, cy + dy * 6.0, cx + dx * sz, cy + dy * sz, 2.5, c);
            }
        }

        self.draw_dmg_dirs(cx, cy);

        if self.vign > 0.0 {
            draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.8, 0.0, 0.0, self.vign * 0.28));
        }
        if !self.palive {
            draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.1, 0.0, 0.0, 0.25));
            let m = measure_text("YOU DIED", None, 44, 1.0);
            draw_text("YOU DIED", cx - m.width / 2.0, sh * 0.18, 44.0, Color::new(1.0, 0.25, 0.2, 0.85));
            let cands = self.spec_candidates();
            if !cands.is_empty() {
                let i = self.current_spec_index();
                let label = format!("Spectating {}  (LMB / Space: next)", cands[i].2);
                let m2 = measure_text(&label, None, 20, 1.0);
                draw_text(&label, cx - m2.width / 2.0, sh - 60.0, 20.0, WHITE);
            }
        }

        // ----- bottom-left: health / armor / money panel -----
        let px = 24.0;
        let py = sh - 92.0;
        draw_rectangle(px - 10.0, py - 8.0, 250.0, 88.0, Color::new(0.0, 0.0, 0.0, 0.42));
        // health
        let hpf = (self.php as f32 / 100.0).clamp(0.0, 1.0);
        let hp_col = Color::new(1.0 - hpf * 0.6, 0.3 + hpf * 0.7, 0.35, 1.0);
        draw_text(&format!("{}", self.php.max(0)), px, py + 26.0, 38.0, hp_col);
        draw_rectangle(px + 78.0, py + 8.0, 150.0, 10.0, Color::new(0.2, 0.05, 0.05, 0.8));
        draw_rectangle(px + 78.0, py + 8.0, 150.0 * hpf, 10.0, hp_col);
        // armor
        let arf = (self.armor as f32 / 100.0).clamp(0.0, 1.0);
        let arm_label = if self.helmet { "ARMOR+" } else { "ARMOR" };
        draw_text(arm_label, px, py + 50.0, 14.0, Color::from_rgba(140, 180, 235, 255));
        draw_rectangle(px + 78.0, py + 40.0, 150.0, 8.0, Color::new(0.05, 0.08, 0.15, 0.8));
        draw_rectangle(px + 78.0, py + 40.0, 150.0 * arf, 8.0, Color::from_rgba(90, 150, 235, 255));
        // money
        draw_text(&format!("${}", self.money), px, py + 72.0, 20.0, Color::from_rgba(120, 220, 130, 255));
        if self.kit {
            draw_text("[KIT]", px + 80.0, py + 72.0, 18.0, Color::from_rgba(120, 220, 235, 255));
        }

        // ----- bottom-right: weapon / ammo + slots -----
        let w = &self.wpn[self.cur];
        let wi = self.loadout[self.cur];
        let wname = WPNS[wi].name;
        let ammo_str = if WPNS[wi].slot == 2 {
            String::new()
        } else if w.reload_t > 0.0 {
            "RELOADING".to_string()
        } else {
            format!("{} / {}", w.mag, w.reserve)
        };
        if !ammo_str.is_empty() {
            let m = measure_text(&ammo_str, None, 40, 1.0);
            draw_text(&ammo_str, sw - m.width - 26.0, sh - 26.0, 40.0, WHITE);
        }
        let m2 = measure_text(wname, None, 20, 1.0);
        draw_text(wname, sw - m2.width - 26.0, sh - 56.0, 20.0, Color::from_rgba(255, 210, 120, 255));
        // slot chips
        let slot_names = ["1", "2", "3"];
        for s in 0..3 {
            let lbl = WPNS[self.loadout[s]].name;
            let txt = format!("{} {}", slot_names[s], lbl);
            let m = measure_text(&txt, None, 15, 1.0);
            let yy = sh - 96.0 - (2 - s) as f32 * 20.0;
            let col = if s == self.cur {
                Color::from_rgba(255, 230, 150, 255)
            } else {
                Color::from_rgba(150, 150, 150, 200)
            };
            draw_text(&txt, sw - m.width - 26.0, yy, 15.0, col);
        }
        if self.i_am_carrier() {
            draw_text("C4", sw - 70.0, sh - 156.0, 24.0, Color::from_rgba(255, 90, 70, 255));
        }

        self.draw_radar();

        let (score_ct, score_t, timer, planted, _phase) = self.hud_numbers();
        let mins = (timer.max(0.0) as i32) / 60;
        let secs = (timer.max(0.0) as i32) % 60;
        let blink = planted && (get_time() * 4.0).sin() > 0.0;
        let tcol = if planted {
            if blink { Color::from_rgba(255, 90, 80, 255) } else { Color::from_rgba(255, 150, 80, 255) }
        } else {
            WHITE
        };
        // central HUD bar: blue CT score | timer | orange T score
        let bw = 240.0;
        draw_rectangle(cx - bw / 2.0, 8.0, bw, 44.0, Color::new(0.0, 0.0, 0.0, 0.55));
        draw_rectangle(cx - bw / 2.0, 8.0, 64.0, 44.0, Color::new(0.10, 0.18, 0.34, 0.85));
        draw_rectangle(cx + bw / 2.0 - 64.0, 8.0, 64.0, 44.0, Color::new(0.34, 0.20, 0.08, 0.85));
        let cts = format!("{}", score_ct);
        let ts = format!("{}", score_t);
        draw_text(&cts, cx - bw / 2.0 + 24.0, 40.0, 30.0, Color::from_rgba(150, 195, 255, 255));
        draw_text(&ts, cx + bw / 2.0 - 44.0, 40.0, 30.0, Color::from_rgba(255, 185, 110, 255));
        let tstr = format!("{}:{:02}", mins, secs);
        let mt = measure_text(&tstr, None, 30, 1.0);
        draw_text(&tstr, cx - mt.width / 2.0, 38.0, 30.0, tcol);
        let rnd = format!("ROUND {}", self.round.max(1));
        let mr = measure_text(&rnd, None, 12, 1.0);
        draw_text(&rnd, cx - mr.width / 2.0, 51.0, 12.0, Color::from_rgba(180, 180, 180, 200));
        if planted {
            let pt = "● BOMB";
            let mp = measure_text(pt, None, 14, 1.0);
            draw_text(pt, cx - mp.width / 2.0, 66.0, 14.0, tcol);
        }

        if self.msg_t > 0.0 && !self.msg.is_empty() {
            let m4 = measure_text(&self.msg, None, 34, 1.0);
            draw_text(&self.msg, cx - m4.width / 2.0, sh * 0.24, 34.0, Color::from_rgba(255, 210, 120, 255));
        }
        if self.sub_t > 0.0 && !self.sub.is_empty() {
            let m5 = measure_text(&self.sub, None, 20, 1.0);
            draw_text(&self.sub, cx - m5.width / 2.0, sh * 0.24 + 30.0, 20.0, Color::from_rgba(220, 220, 220, 255));
        }

        let mut ky = 64.0;
        for (txt, col, life) in &self.killfeed {
            let a = (life / 1.0).clamp(0.0, 1.0);
            let m6 = measure_text(txt, None, 18, 1.0);
            draw_rectangle(sw - m6.width - 34.0, ky - 16.0, m6.width + 18.0, 22.0, Color::new(0.0, 0.0, 0.0, 0.45 * a));
            draw_text(txt, sw - m6.width - 25.0, ky, 18.0, Color::new(col.r, col.g, col.b, a));
            ky += 26.0;
        }

        // interaction bar: authority uses local timers, client uses snapshot progress
        let bar_y = sh * 0.68;
        let (plant_frac, defuse_frac) = if self.is_authority() {
            (self.plant_t / PLANT_TIME, self.defuse_t / DEFUSE_TIME)
        } else {
            let p = self
                .cstate
                .snap
                .as_ref()
                .and_then(|s| s.players.iter().find(|p| p.id == self.cstate.my_id))
                .map_or(0.0, |p| p.progress);
            if self.player_team == Team::T {
                (p, 0.0)
            } else {
                (0.0, p)
            }
        };
        if defuse_frac > 0.0 {
            self.draw_progress(cx, bar_y, "DEFUSING", defuse_frac);
        } else if plant_frac > 0.0 {
            self.draw_progress(cx, bar_y, "PLANTING", plant_frac);
        } else if self.palive && self.local_phase_live() {
            if let Some((bp, _, defused)) = self.bomb_view() {
                if self.player_team == Team::Ct && !defused && self.ppos.distance(bp) < 2.4 {
                    let hint = "Hold E to defuse";
                    let m7 = measure_text(hint, None, 22, 1.0);
                    draw_text(hint, cx - m7.width / 2.0, bar_y, 22.0, WHITE);
                }
            } else if self.player_team == Team::T && self.i_am_carrier() && self.near_any_site() {
                let hint = "Hold E to plant the bomb";
                let m7 = measure_text(hint, None, 22, 1.0);
                draw_text(hint, cx - m7.width / 2.0, bar_y, 22.0, WHITE);
            }
        }

        draw_text(&format!("{} fps", get_fps()), 10.0, 20.0, 16.0, Color::new(1.0, 1.0, 1.0, 0.5));
        match &self.role {
            Role::Host(_) => {
                let n = self.remotes.len();
                draw_text(
                    &format!("HOSTING - {} connected (ticket in clipboard)", n),
                    10.0,
                    38.0,
                    14.0,
                    Color::new(0.6, 1.0, 0.6, 0.7),
                );
            }
            Role::Client(_) => {
                if let Some(why) = &self.cstate.disconnect {
                    let m = measure_text("DISCONNECTED", None, 40, 1.0);
                    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.6));
                    draw_text("DISCONNECTED", cx - m.width / 2.0, cy - 20.0, 40.0, RED);
                    let m2 = measure_text(why, None, 18, 1.0);
                    draw_text(why, cx - m2.width / 2.0, cy + 10.0, 18.0, WHITE);
                }
            }
            Role::Solo => {}
        }

        if self.dbg_open {
            let items = [
                ("F1 god", self.dbg_god),
                ("F2 noclip", self.dbg_noclip),
                ("F3 wallhack", self.dbg_esp),
                ("F4 bot paths", self.dbg_paths),
                ("F5 freeze AI", self.dbg_freeze),
                ("F6 kill enemies", false),
                ("F7 heal + ammo", false),
                ("F8 uncap fps", self.dbg_uncap),
            ];
            draw_rectangle(8.0, 46.0, 170.0, 24.0 + items.len() as f32 * 20.0, Color::new(0.0, 0.0, 0.0, 0.6));
            draw_text("DEBUG (F10)", 16.0, 64.0, 16.0, YELLOW);
            for (k, (label, on)) in items.iter().enumerate() {
                let col = if *on { GREEN } else { Color::from_rgba(200, 200, 200, 255) };
                let state = if *on { " ON" } else { "" };
                draw_text(&format!("{label}{state}"), 16.0, 84.0 + k as f32 * 20.0, 15.0, col);
            }
        }

        // freeze-time buy hint
        if self.local_phase_live() == false && self.palive && self.phase != Phase::Menu && !self.buy_open {
            let hint = "Press B to buy";
            let m = measure_text(hint, None, 20, 1.0);
            draw_text(hint, cx - m.width / 2.0, sh * 0.6, 20.0, Color::from_rgba(120, 230, 140, 220));
        }

        if self.buy_open {
            self.draw_buy_menu();
        }
        if self.show_scores {
            self.draw_scoreboard();
        }

        if !self.grabbed && self.phase != Phase::Menu && !self.buy_open {
            let m8 = measure_text("PAUSED - click to resume", None, 30, 1.0);
            draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.45));
            draw_text("PAUSED - click to resume", cx - m8.width / 2.0, cy, 30.0, WHITE);
        }
    }

    // ---------- HUD sub-widgets ----------

    fn draw_dmg_numbers(&self, cam: &Camera3D) {
        for d in &self.dmg_nums {
            if let Some(s) = self.project(cam, d.pos) {
                let f = (d.life / 0.9).clamp(0.0, 1.0);
                let col = if d.head {
                    Color::new(1.0, 0.85, 0.3, f)
                } else {
                    Color::new(1.0, 1.0, 1.0, f)
                };
                let sz = if d.head { 26.0 } else { 20.0 };
                let txt = format!("{}", d.amount);
                let m = measure_text(&txt, None, sz as u16, 1.0);
                draw_text(&txt, s.x - m.width / 2.0, s.y, sz, col);
            }
        }
    }

    fn draw_dmg_dirs(&self, cx: f32, cy: f32) {
        for (yaw, life) in &self.dmg_dir {
            let f = (life / 1.4).clamp(0.0, 1.0);
            // angle of source relative to where we look
            let rel = yaw - self.yaw;
            let (s, c) = rel.sin_cos();
            let r = 90.0;
            let dx = s;
            let dy = -c;
            let bx = cx + dx * r;
            let by = cy + dy * r;
            let col = Color::new(1.0, 0.2, 0.2, 0.8 * f);
            // little arc/arrow pointing inward
            let perp = vec2(-dy, dx);
            let p0 = vec2(bx, by) + perp * 22.0;
            let p1 = vec2(bx, by) - perp * 22.0;
            let tip = vec2(bx, by) - vec2(dx, dy) * 14.0;
            draw_triangle(p0, p1, tip, col);
        }
    }

    fn draw_radar(&self) {
        let sw = screen_width();
        let r = 92.0;
        let ox = 24.0 + r;
        let oy = 24.0 + r;
        let _ = sw;
        // background disc
        draw_circle(ox, oy, r, Color::new(0.04, 0.06, 0.05, 0.7));
        draw_circle_lines(ox, oy, r, 2.0, Color::new(0.4, 0.7, 0.5, 0.6));
        // map extent ~ x in [-41,41], z in [-31,31]; scale to radar
        let scale = (r - 8.0) / 41.0;
        let me_yaw = self.yaw;
        let to_radar = |wx: f32, wz: f32| {
            // rotate world so player's facing points up
            let dx = wx - self.ppos.x;
            let dz = wz - self.ppos.z;
            let (s, c) = me_yaw.sin_cos();
            let rx = dx * c - dz * s;
            let rz = dx * s + dz * c;
            vec2(ox + rx * scale, oy + rz * scale)
        };
        // site markers
        for (site, lbl) in [(SITE_A, "A"), (SITE_B, "B")] {
            let p = to_radar(site.x, site.y);
            if p.distance(vec2(ox, oy)) < r - 2.0 {
                draw_text(lbl, p.x - 4.0, p.y + 4.0, 16.0, Color::new(1.0, 0.8, 0.4, 0.7));
            }
        }
        // bomb
        if let Some((bp, _, def)) = self.bomb_view() {
            if !def {
                let p = to_radar(bp.x, bp.z);
                let blink = (get_time() * 5.0).sin() > 0.0;
                if blink {
                    draw_circle(p.x, p.y, 4.0, Color::from_rgba(255, 60, 50, 255));
                }
            }
        }
        // teammates + enemies (only those known to client/authority)
        for e in self.collect_draw_ents() {
            let p = to_radar(e.pos.x, e.pos.z);
            if p.distance(vec2(ox, oy)) > r - 3.0 {
                continue;
            }
            let friend = e.team == self.player_team;
            let col = if friend {
                Color::from_rgba(90, 200, 110, 230)
            } else {
                Color::from_rgba(230, 80, 70, 230)
            };
            // enemies only show on radar if roughly in view or a teammate (simplified intel)
            if !friend {
                let to = vec3(e.pos.x - self.ppos.x, 0.0, e.pos.z - self.ppos.z).normalize_or_zero();
                if self.flat_forward().dot(to) < 0.2 {
                    continue;
                }
            }
            draw_circle(p.x, p.y, 3.0, col);
        }
        // self marker (arrow pointing up)
        draw_triangle(
            vec2(ox, oy - 6.0),
            vec2(ox - 4.0, oy + 4.0),
            vec2(ox + 4.0, oy + 4.0),
            Color::from_rgba(240, 240, 240, 255),
        );
    }

    fn draw_scoreboard(&self) {
        let sw = screen_width();
        let sh = screen_height();
        let cx = sw / 2.0;
        let pw = 720.0;
        let px = cx - pw / 2.0;
        let py = sh * 0.16;
        draw_rectangle(px, py, pw, sh * 0.68, Color::new(0.03, 0.05, 0.08, 0.9));
        draw_rectangle(px, py, pw, 40.0, Color::new(0.08, 0.12, 0.2, 0.95));
        let (sc, st, _, _, _) = self.hud_numbers();
        let title = format!("COUNTER-TERRORISTS  {}   :   {}  TERRORISTS", sc, st);
        let mt = measure_text(&title, None, 24, 1.0);
        draw_text(&title, cx - mt.width / 2.0, py + 28.0, 24.0, WHITE);

        // gather rows per team
        let mut rows: Vec<(Team, String, i32, i32, bool)> = Vec::new();
        // me
        rows.push((self.player_team, format!("{} (you)", self.my_name), self.my_kills, self.my_deaths, self.palive));
        for r in &self.remotes {
            rows.push((r.team, r.name.clone(), r.kills, r.deaths, r.alive));
        }
        for b in &self.bots {
            rows.push((b.team, format!("{} [bot]", b.name), b.kills, b.deaths, b.alive));
        }

        for (col, team, header_x) in [
            (Color::from_rgba(140, 190, 255, 255), Team::Ct, px + 20.0),
            (Color::from_rgba(255, 180, 110, 255), Team::T, cx + 20.0),
        ] {
            let mut y = py + 70.0;
            draw_text("PLAYER", header_x, y, 16.0, col);
            draw_text("K", header_x + 250.0, y, 16.0, col);
            draw_text("D", header_x + 290.0, y, 16.0, col);
            y += 26.0;
            let mut list: Vec<&(Team, String, i32, i32, bool)> =
                rows.iter().filter(|r| r.0 == team).collect();
            list.sort_by(|a, b| b.2.cmp(&a.2));
            for row in list {
                let a = if row.4 { 1.0 } else { 0.4 };
                let rc = Color::new(col.r, col.g, col.b, a);
                draw_text(&row.1, header_x, y, 18.0, rc);
                draw_text(&format!("{}", row.2), header_x + 250.0, y, 18.0, Color::new(1.0, 1.0, 1.0, a));
                draw_text(&format!("{}", row.3), header_x + 290.0, y, 18.0, Color::new(0.8, 0.8, 0.8, a));
                y += 24.0;
            }
        }
    }

    fn buy_items(&self) -> [(KeyCode, &'static str, usize, i32); 6] {
        [
            (KeyCode::Key1, "Kevlar + Helmet", usize::MAX, 1000),
            (KeyCode::Key2, "Defuse Kit", usize::MAX - 1, 400),
            (KeyCode::Key3, "MP5 SMG", W_MP5, WPNS[W_MP5].price),
            (
                KeyCode::Key4,
                if self.player_team == Team::T { "AK-47" } else { "M4A1-S" },
                if self.player_team == Team::T { W_AK } else { W_M4 },
                WPNS[if self.player_team == Team::T { W_AK } else { W_M4 }].price,
            ),
            (KeyCode::Key5, "AWP Sniper", W_AWP, WPNS[W_AWP].price),
            (KeyCode::Key6, "Deagle", W_DEAGLE, WPNS[W_DEAGLE].price),
        ]
    }

    fn draw_buy_menu(&self) {
        let sw = screen_width();
        let sh = screen_height();
        let cx = sw / 2.0;
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.02, 0.03, 0.05, 0.78));
        let pw = 460.0;
        let px = cx - pw / 2.0;
        let py = sh * 0.22;
        draw_rectangle(px, py, pw, 360.0, Color::new(0.06, 0.08, 0.12, 0.95));
        draw_text("BUY MENU", px + 24.0, py + 38.0, 30.0, Color::from_rgba(255, 210, 120, 255));
        draw_text(&format!("${}", self.money), px + pw - 130.0, py + 38.0, 28.0, Color::from_rgba(120, 220, 130, 255));
        let mut y = py + 86.0;
        for (key, name, idx, price) in self.buy_items() {
            let owned = match idx {
                usize::MAX => self.armor >= 100 && self.helmet,
                i if i == usize::MAX - 1 => self.kit,
                i => self.loadout[0] == i || self.loadout[1] == i,
            };
            let afford = self.money >= price;
            let kc = match key {
                KeyCode::Key1 => "1",
                KeyCode::Key2 => "2",
                KeyCode::Key3 => "3",
                KeyCode::Key4 => "4",
                KeyCode::Key5 => "5",
                _ => "6",
            };
            let col = if owned {
                Color::from_rgba(120, 120, 120, 255)
            } else if afford {
                WHITE
            } else {
                Color::from_rgba(180, 90, 90, 255)
            };
            let tag = if owned { " (owned)".to_string() } else { format!("  ${}", price) };
            draw_text(&format!("[{}]  {}{}", kc, name, tag), px + 24.0, y, 22.0, col);
            y += 40.0;
        }
        draw_text(
            "B / Esc to close   ·   buy during freeze time",
            px + 24.0,
            py + 332.0,
            16.0,
            Color::from_rgba(180, 180, 180, 220),
        );
    }

    fn buy(&mut self, idx: usize, price: i32) {
        if self.money < price {
            play(&self.snd.dryfire, 0.4);
            return;
        }
        match idx {
            usize::MAX => {
                if self.armor >= 100 && self.helmet {
                    return;
                }
                self.armor = 100;
                self.helmet = true;
            }
            i if i == usize::MAX - 1 => {
                if self.player_team != Team::Ct || self.kit {
                    play(&self.snd.dryfire, 0.4);
                    return;
                }
                self.kit = true;
            }
            i => {
                let slot = WPNS[i].slot as usize;
                self.loadout[slot] = i;
                let d = &WPNS[i];
                self.wpn[slot] = WpnState { mag: d.mag_size, reserve: d.reserve_start, reload_t: 0.0, cd: 0.0 };
                self.cur = slot;
            }
        }
        self.money -= price;
        play(&self.snd.pickup, 0.5);
    }

    fn current_spec_index(&self) -> usize {
        let n = self.spec_candidates().len();
        if n == 0 {
            0
        } else {
            self.spec_i % n
        }
    }

    fn draw_progress(&self, cx: f32, y: f32, label: &str, frac: f32) {
        let bw = 280.0;
        let m = measure_text(label, None, 20, 1.0);
        draw_text(label, cx - m.width / 2.0, y - 12.0, 20.0, WHITE);
        draw_rectangle(cx - bw / 2.0, y, bw, 12.0, Color::new(0.0, 0.0, 0.0, 0.6));
        draw_rectangle(cx - bw / 2.0, y, bw * frac.clamp(0.0, 1.0), 12.0, Color::from_rgba(80, 255, 80, 255));
    }

    fn draw_menu(&self) {
        let sw = screen_width();
        let sh = screen_height();
        let cx = sw / 2.0;
        // cinematic letterbox + soft darkening that still reveals the orbiting arena
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.02, 0.03, 0.05, 0.32));
        draw_rectangle(0.0, 0.0, sw, sh * 0.06, BLACK);
        draw_rectangle(0.0, sh * 0.94, sw, sh * 0.06, BLACK);
        // top + bottom gradient bands for readability
        for k in 0..40 {
            let a = 0.6 * (1.0 - k as f32 / 40.0);
            draw_rectangle(0.0, sh * 0.06 + k as f32 * 3.0, sw, 3.0, Color::new(0.0, 0.0, 0.0, a * 0.5));
            draw_rectangle(0.0, sh * 0.94 - k as f32 * 4.0, sw, 4.0, Color::new(0.0, 0.0, 0.0, a));
        }
        let title = "OPERATION: MICRO";
        let mt = measure_text(title, None, 64, 1.0);
        draw_text(title, cx - mt.width / 2.0 + 2.0, sh * 0.2 + 2.0, 64.0, Color::new(0.0, 0.0, 0.0, 0.6));
        draw_text(title, cx - mt.width / 2.0, sh * 0.2, 64.0, Color::from_rgba(255, 214, 130, 255));
        let tag = "TACTICAL DEFUSE  ·  ONE MAP  ·  AAA DEMO";
        let mtg = measure_text(tag, None, 18, 1.0);
        draw_text(tag, cx - mtg.width / 2.0, sh * 0.2 + 26.0, 18.0, Color::from_rgba(180, 200, 220, 220));
        let sub = match &self.role {
            Role::Solo => "Defuse mode vs bots - first to 8 rounds".to_string(),
            Role::Host(_) => match &self.ticket {
                Some(_) => "HOSTING - ticket copied to clipboard, send it to friends".to_string(),
                None => "HOSTING - getting ticket...".to_string(),
            },
            Role::Client(_) => "JOINING - pick a team, then click".to_string(),
        };
        let ms = measure_text(&sub, None, 24, 1.0);
        draw_text(&sub, cx - ms.width / 2.0, sh * 0.22 + 36.0, 24.0, Color::from_rgba(180, 220, 180, 255));

        if let Some(t) = &self.ticket {
            let short = format!("{}...{}", &t[..16.min(t.len())], &t[t.len().saturating_sub(8)..]);
            let line = format!("ticket: {short}");
            let m = measure_text(&line, None, 16, 1.0);
            draw_text(&line, cx - m.width / 2.0, sh * 0.22 + 62.0, 16.0, GRAY);
        }

        let ct_sel = self.player_team == Team::Ct;
        let ct_label = if ct_sel { "> [1] Counter-Terrorists <" } else { "[1] Counter-Terrorists" };
        let t_label = if !ct_sel { "> [2] Terrorists <" } else { "[2] Terrorists" };
        let mc = measure_text(ct_label, None, 28, 1.0);
        draw_text(
            ct_label,
            cx - mc.width / 2.0,
            sh * 0.4,
            28.0,
            if ct_sel { Color::from_rgba(110, 170, 255, 255) } else { GRAY },
        );
        let mt2 = measure_text(t_label, None, 28, 1.0);
        draw_text(
            t_label,
            cx - mt2.width / 2.0,
            sh * 0.4 + 34.0,
            28.0,
            if !ct_sel { Color::from_rgba(255, 160, 70, 255) } else { GRAY },
        );

        if self.is_authority() {
            let ff_label = format!("[F] friendly fire: {}", if self.ff { "ON" } else { "OFF" });
            let bots_label = format!("[Up/Down] bots fill teams to: {}", self.bot_fill);
            let mf = measure_text(&ff_label, None, 22, 1.0);
            draw_text(
                &ff_label,
                cx - mf.width / 2.0,
                sh * 0.4 + 78.0,
                22.0,
                if self.ff { Color::from_rgba(255, 120, 120, 255) } else { GRAY },
            );
            let mb = measure_text(&bots_label, None, 22, 1.0);
            draw_text(&bots_label, cx - mb.width / 2.0, sh * 0.4 + 106.0, 22.0, Color::from_rgba(210, 210, 210, 255));
        }

        let online_line = if matches!(self.role, Role::Solo) {
            "[H] host online game    ·    [V] join game (ticket in clipboard)"
        } else {
            "F10 debug menu in-game"
        };
        let lines = [
            "WASD move   ·   Mouse aim/shoot   ·   RMB scope (AWP)   ·   Space jump   ·   Shift walk",
            "1/2/3 weapons   ·   R reload   ·   B buy   ·   E plant/defuse   ·   Tab scores   ·   Esc pause",
            online_line,
        ];
        let mut y = sh * 0.62;
        for l in lines {
            let m = measure_text(l, None, 19, 1.0);
            draw_text(l, cx - m.width / 2.0, y, 19.0, Color::from_rgba(214, 214, 214, 235));
            y += 28.0;
        }
        if self.sub_t > 0.0 && !self.sub.is_empty() {
            let m = measure_text(&self.sub, None, 20, 1.0);
            draw_text(&self.sub, cx - m.width / 2.0, sh * 0.72, 20.0, Color::from_rgba(255, 150, 130, 255));
        }
        let go = if matches!(self.role, Role::Client(_)) && !self.cstate.welcomed {
            "CLICK TO CONNECT"
        } else {
            "CLICK TO PLAY"
        };
        let mg = measure_text(go, None, 30, 1.0);
        let pulse = ((get_time() * 3.0).sin() * 0.3 + 0.7) as f32;
        draw_text(go, cx - mg.width / 2.0, sh * 0.78, 30.0, Color::new(0.3, 1.0, 0.3, pulse));
    }

    fn debug_input(&mut self) {
        if is_key_pressed(KeyCode::F10) {
            self.dbg_open = !self.dbg_open;
        }
        if !self.dbg_open {
            return;
        }
        if is_key_pressed(KeyCode::F1) {
            self.dbg_god = !self.dbg_god;
        }
        if is_key_pressed(KeyCode::F2) {
            self.dbg_noclip = !self.dbg_noclip;
        }
        if is_key_pressed(KeyCode::F3) {
            self.dbg_esp = !self.dbg_esp;
        }
        if is_key_pressed(KeyCode::F4) {
            self.dbg_paths = !self.dbg_paths;
        }
        if is_key_pressed(KeyCode::F5) {
            self.dbg_freeze = !self.dbg_freeze;
        }
        if is_key_pressed(KeyCode::F6) && self.is_authority() {
            let enemy = if self.player_team == Team::Ct { Team::T } else { Team::Ct };
            for i in 0..self.bots.len() {
                if self.bots[i].alive && self.bots[i].team == enemy {
                    self.kill_bot(i, "Debug".to_string(), self.player_team);
                }
            }
        }
        if is_key_pressed(KeyCode::F7) {
            self.php = 100;
            self.armor = 100;
            self.helmet = true;
            self.refill_loadout();
        }
        if is_key_pressed(KeyCode::F8) {
            self.dbg_uncap = !self.dbg_uncap;
        }
    }
}

fn conf() -> Conf {
    Conf {
        window_title: "de_micro".to_string(),
        window_width: 1440,
        window_height: 900,
        high_dpi: true,
        sample_count: 1,
        platform: macroquad::miniquad::conf::Platform {
            blocking_event_loop: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[macroquad::main(conf)]
async fn main() {
    rand::srand(macroquad::miniquad::date::now() as u64);

    let args: Vec<String> = std::env::args().collect();
    let mut role_arg = 0; // 0 solo, 1 host
    let mut join_ticket: Option<String> = None;
    let mut name = std::env::var("USER").unwrap_or_else(|_| "Player".into());
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => role_arg = 1,
            "--join" => {
                if i + 1 < args.len() {
                    join_ticket = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--name" => {
                if i + 1 < args.len() {
                    name = args[i + 1].clone();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let snd = Sounds::load().await;
    let (walls, meshes) = build_map();
    let nav = Nav::build(&walls);

    let client_wants_t = args.iter().any(|a| a == "--team-t" || a == "-t");
    let role = if let Some(t) = &join_ticket {
        Role::Client(net::start_client(t.clone(), name.clone(), client_wants_t))
    } else if role_arg == 1 {
        Role::Host(net::start_host())
    } else {
        Role::Solo
    };

    let mut g = Game {
        walls,
        meshes,
        nav,
        snd,
        role,
        my_name: name,
        ff: false,
        bot_fill: 4,
        ticket: None,
        cstate: CState {
            my_id: 255,
            welcomed: false,
            snap: None,
            step_acc: Default::default(),
            spawn_seq: 0,
            beep_t: 0.0,
            prev_hp: 100,
            prev_alive: true,
            disconnect: None,
        },
        remotes: Vec::new(),
        ev_buf: Vec::new(),
        snap_timer: 0.0,
        input_timer: 0.0,
        player_team: if std::env::args().any(|a| a == "--team-t" || a == "-t") {
            Team::T
        } else {
            Team::Ct
        },
        ppos: vec3(0.0, 0.0, 26.0),
        pvel: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        php: 100,
        palive: true,
        on_ground: true,
        cur: 0,
        loadout: [W_AK, W_USP, W_KNIFE],
        wpn: [WpnState {
            mag: 30,
            reserve: 90,
            reload_t: 0.0,
            cd: 0.0,
        }; 3],
        spread_add: 0.0,
        bob: 0.0,
        switch_t: 0.0,
        muzzle_t: 0.0,
        defuse_t: 0.0,
        plant_t: 0.0,
        step_t: 0.0,
        spec_i: 0,
        punch: Vec2::ZERO,
        recoil_i: 0,
        land_t: 0.0,
        zoom: 0.0,
        money: 800,
        armor: 0,
        helmet: false,
        kit: false,
        buy_open: false,
        my_kills: 0,
        my_deaths: 0,
        pending_buy_primary: W_AK,
        pending_buy_secondary: W_USP,
        show_scores: false,
        menu_t: 0.0,
        bots: Vec::new(),
        bomb: None,
        defused: false,
        carrier: None,
        dropped: None,
        site: SITE_A,
        phase: Phase::Menu,
        phase_t: 0.0,
        round_time: ROUND_TIME,
        round: 0,
        score_ct: 0,
        score_t: 0,
        match_over: false,
        msg: String::new(),
        msg_t: 0.0,
        sub: String::new(),
        sub_t: 0.0,
        killfeed: Vec::new(),
        tracers: Vec::new(),
        flashes: Vec::new(),
        explosion: None,
        particles: Vec::new(),
        decals: Vec::new(),
        corpses: Vec::new(),
        dmg_nums: Vec::new(),
        dmg_dir: Vec::new(),
        vign: 0.0,
        hitm: 0.0,
        hitk: false,
        shake: 0.0,
        dbg_open: false,
        dbg_god: false,
        dbg_noclip: false,
        dbg_esp: false,
        dbg_paths: false,
        dbg_freeze: false,
        dbg_uncap: false,
        grabbed: false,
        last_mouse: mouse_position().into(),
    };

    // the detailed floor/world meshes exceed macroquad's default per-drawcall
    // buffer (10k verts / 5k indices); raise it so nothing is clipped
    macroquad::window::gl_set_drawcall_buffer_capacity(120_000, 120_000);

    let mut fps_log = 0u32;
    let mut next_deadline = std::time::Instant::now();
    play_looped(&g.snd.ambient, 0.16);

    loop {
        macroquad::miniquad::window::schedule_update();
        let dt = get_frame_time().min(0.05);
        let t_now = get_time();

        fps_log += 1;
        if fps_log % 600 == 0 {
            println!("{} fps", get_fps());
        }

        // network polling always runs (even paused / in menu)
        g.host_poll(dt);
        g.client_poll();

        if is_key_pressed(KeyCode::Escape) && g.grabbed && !g.buy_open {
            g.grabbed = false;
            set_cursor_grab(false);
            show_mouse(true);
        }

        // buy menu + scoreboard (in-game only)
        if g.phase != Phase::Menu {
            g.show_scores = g.grabbed && is_key_down(KeyCode::Tab);
            let can_buy = g.palive && g.grabbed && !g.local_phase_live();
            if is_key_pressed(KeyCode::B) && can_buy {
                g.buy_open = !g.buy_open;
            }
            if g.local_phase_live() {
                g.buy_open = false;
            }
            if g.buy_open {
                for (key, _, idx, price) in g.buy_items() {
                    if is_key_pressed(key) {
                        g.buy(idx, price);
                    }
                }
                if is_key_pressed(KeyCode::B) || is_key_pressed(KeyCode::Escape) {
                    g.buy_open = false;
                }
            }
        }

        if g.phase == Phase::Menu {
            if is_key_pressed(KeyCode::Key1) {
                g.player_team = Team::Ct;
                play(&g.snd.ui_click, 0.5);
            }
            if is_key_pressed(KeyCode::Key2) {
                g.player_team = Team::T;
                play(&g.snd.ui_click, 0.5);
            }
            if g.is_authority() {
                if is_key_pressed(KeyCode::F) {
                    g.ff = !g.ff;
                    play(&g.snd.ui_click, 0.5);
                }
                if is_key_pressed(KeyCode::Up) {
                    g.bot_fill = (g.bot_fill + 1).min(4);
                    play(&g.snd.ui_hover, 0.5);
                }
                if is_key_pressed(KeyCode::Down) {
                    g.bot_fill = (g.bot_fill - 1).max(0);
                    play(&g.snd.ui_hover, 0.5);
                }
            }
            if matches!(g.role, Role::Solo) {
                if is_key_pressed(KeyCode::H) {
                    g.role = Role::Host(net::start_host());
                }
                if is_key_pressed(KeyCode::V) {
                    match macroquad::miniquad::window::clipboard_get() {
                        Some(t) if net::addr_from_ticket(&t).is_some() => {
                            g.role = Role::Client(net::start_client(
                                t,
                                g.my_name.clone(),
                                g.player_team.is_t(),
                            ));
                        }
                        _ => {
                            g.show_sub("clipboard does not contain a valid ticket", 3.0);
                        }
                    }
                }
            }
        }

        if is_mouse_button_pressed(MouseButton::Left) && !g.grabbed {
            g.grabbed = true;
            set_cursor_grab(true);
            show_mouse(false);
            g.last_mouse = mouse_position().into();
            if g.phase == Phase::Menu && g.is_authority() {
                g.start_round();
            } else if g.phase == Phase::Menu {
                // client: leave menu; gameplay state driven by snapshots
                g.phase = Phase::Live;
            }
        } else if g.grabbed && !g.palive && g.phase != Phase::Menu {
            if is_mouse_button_pressed(MouseButton::Left) || is_key_pressed(KeyCode::Space) {
                g.spec_i = g.spec_i.wrapping_add(1);
            }
        }

        let mp: Vec2 = mouse_position().into();
        let mdelta = mp - g.last_mouse;
        g.last_mouse = mp;
        if g.grabbed && g.palive && g.phase != Phase::Menu && !g.buy_open {
            let sens = 0.0028 * (1.0 - g.zoom * 0.65);
            g.yaw -= mdelta.x * sens;
            g.pitch = (g.pitch - mdelta.y * sens).clamp(-1.45, 1.45);
        }

        g.debug_input();

        if g.phase == Phase::Menu {
            g.sub_t -= dt;
            g.msg_t -= dt;
            g.menu_t += dt;
            g.tick_fx(dt);
        }

        let paused = !g.grabbed && g.phase != Phase::Menu && matches!(g.role, Role::Solo);

        if !paused && g.phase != Phase::Menu {
            if g.is_authority() {
                match g.phase {
                    Phase::Freeze => {
                        g.phase_t -= dt;
                        g.remotes_update(dt);
                        if g.phase_t <= 0.0 {
                            g.phase = Phase::Live;
                            g.show_msg("GO! GO! GO!", 1.0);
                            play(&g.snd.whistle, 0.6);
                        }
                    }
                    Phase::Live => {
                        if g.bomb.is_none() {
                            g.round_time -= dt;
                        }
                        g.player_update(dt);
                        g.remotes_update(dt);
                        if !g.dbg_freeze {
                            g.bots_update(dt);
                        }
                        g.bomb_update(dt);
                        g.check_round_end();
                    }
                    Phase::Post => {
                        g.phase_t -= dt;
                        g.player_update(dt);
                        g.remotes_update(dt);
                        g.bomb_update(dt);
                        if g.phase_t <= 0.0 {
                            g.start_round();
                        }
                    }
                    Phase::Menu => {}
                }
                g.host_broadcast(dt);
            } else {
                // client: local movement + send inputs; world state from snapshots
                if g.cstate.disconnect.is_none() {
                    g.player_update(dt);
                    g.client_send_input(dt);
                    // bomb beeps from snapshot
                    if let Some((bp, t, defused)) = g.bomb_view() {
                        if !defused {
                            g.cstate.beep_t -= dt;
                            if g.cstate.beep_t <= 0.0 {
                                g.cstate.beep_t = 0.12 + (t / BOMB_TIME).max(0.0) * 0.95;
                                play(&g.snd.beep, dist_vol(g.ppos, bp, 0.6).max(0.12));
                            }
                        }
                    }
                }
            }

            g.msg_t -= dt;
            g.sub_t -= dt;
            g.vign = (g.vign - dt * 1.2).max(0.0);
            g.hitm -= dt;
            g.muzzle_t -= dt;
            g.shake = (g.shake - dt * 0.15).max(0.0);
            for tr in &mut g.tracers {
                tr.2 -= dt;
            }
            g.tracers.retain(|t| t.2 > 0.0);
            for f in &mut g.flashes {
                f.1 -= dt;
            }
            g.flashes.retain(|f| f.1 > 0.0);
            if let Some((_, age)) = &mut g.explosion {
                *age += dt;
            }
            if g.explosion.map_or(false, |(_, a)| a > 0.7) {
                g.explosion = None;
            }
            for k in &mut g.killfeed {
                k.2 -= dt;
            }
            g.killfeed.retain(|k| k.2 > 0.0);
            g.tick_fx(dt);
        } else if g.phase == Phase::Menu && matches!(g.role, Role::Host(_)) {
            // host can sit in menu while clients wait; keep broadcasting empty-ish snaps
            g.host_broadcast(dt);
        }

        clear_background(Color::from_rgba(189, 204, 219, 255));
        let shake_off = if g.shake > 0.0 {
            vec3(gen_range(-g.shake, g.shake), gen_range(-g.shake, g.shake), 0.0)
        } else {
            Vec3::ZERO
        };
        let (eye, dir) = if g.phase == Phase::Menu {
            // slow cinematic orbit of the arena behind the menu
            let a = g.menu_t * 0.12;
            let orbit = vec3(a.cos() * 30.0, 14.0 + (g.menu_t * 0.3).sin() * 2.0, a.sin() * 30.0);
            let look = (vec3(0.0, 3.0, 0.0) - orbit).normalize_or_zero();
            (orbit, look)
        } else if !g.palive {
            let cands = g.spec_candidates();
            if cands.is_empty() {
                (g.player_eye() + shake_off, g.look_dir())
            } else {
                let i = g.current_spec_index();
                let (e, yaw, _) = &cands[i];
                (*e + shake_off, vec3(-yaw.sin(), 0.0, -yaw.cos()))
            }
        } else {
            (g.player_eye() + shake_off, g.look_dir())
        };
        let cam = Camera3D {
            position: eye,
            target: eye + dir,
            up: vec3(0.0, 1.0, 0.0),
            fovy: (80.0 - g.zoom * 52.0).to_radians(),
            ..Default::default()
        };
        set_camera(&cam);
        g.draw_world(t_now, &cam);
        g.draw_viewmodel();
        set_default_camera();
        if g.phase == Phase::Menu {
            g.draw_menu();
        } else {
            g.draw_hud(&cam);
        }

        if !g.dbg_uncap {
            next_deadline += std::time::Duration::from_micros(16_666);
            let now = std::time::Instant::now();
            if now < next_deadline {
                std::thread::sleep(next_deadline - now);
            } else {
                next_deadline = now;
            }
        } else {
            next_deadline = std::time::Instant::now();
        }

        next_frame().await;
    }
}
