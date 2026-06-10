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
    pistol: Sound,
    far: Sound,
    hit: Sound,
    reload: Sound,
    beep: Sound,
    planted: Sound,
    defused: Sound,
    explosion: Sound,
    death: Sound,
    step: Sound,
}

impl Sounds {
    async fn load() -> Sounds {
        async fn s(samples: Vec<f32>) -> Sound {
            load_sound_from_bytes(&wav_bytes(&samples)).await.unwrap()
        }
        Sounds {
            ak: s(gen_shot(0.13, 0.38, 0.9, 1234)).await,
            pistol: s(gen_shot(0.09, 0.55, 0.75, 4321)).await,
            far: s(gen_shot(0.18, 0.07, 0.6, 9999)).await,
            hit: s(gen_beep(1700.0, 0.05, 0.4)).await,
            reload: s(gen_shot(0.04, 0.85, 0.45, 55)).await,
            beep: s(gen_beep(980.0, 0.09, 0.5)).await,
            planted: s(concat(gen_beep(1318.0, 0.12, 0.5), gen_beep(1568.0, 0.18, 0.5))).await,
            defused: s(concat(gen_beep(880.0, 0.15, 0.5), gen_beep(1318.0, 0.25, 0.5))).await,
            explosion: s(gen_explosion()).await,
            death: s(gen_shot(0.3, 0.05, 0.7, 31337)).await,
            step: s(gen_shot(0.045, 0.16, 0.5, 222)).await,
        }
    }
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

fn push_box(verts: &mut Vec<Vertex>, inds: &mut Vec<u16>, b: &Aabb, c: Color) {
    let (mn, mx) = (b.min, b.max);
    push_quad(
        verts,
        inds,
        [
            vec3(mn.x, mx.y, mn.z),
            vec3(mx.x, mx.y, mn.z),
            vec3(mx.x, mx.y, mx.z),
            vec3(mn.x, mx.y, mx.z),
        ],
        shade(c, 1.0),
    );
    push_quad(
        verts,
        inds,
        [
            vec3(mn.x, mn.y, mn.z),
            vec3(mx.x, mn.y, mn.z),
            vec3(mx.x, mx.y, mn.z),
            vec3(mn.x, mx.y, mn.z),
        ],
        shade(c, 0.82),
    );
    push_quad(
        verts,
        inds,
        [
            vec3(mn.x, mn.y, mx.z),
            vec3(mx.x, mn.y, mx.z),
            vec3(mx.x, mx.y, mx.z),
            vec3(mn.x, mx.y, mx.z),
        ],
        shade(c, 0.74),
    );
    push_quad(
        verts,
        inds,
        [
            vec3(mn.x, mn.y, mn.z),
            vec3(mn.x, mn.y, mx.z),
            vec3(mn.x, mx.y, mx.z),
            vec3(mn.x, mx.y, mn.z),
        ],
        shade(c, 0.6),
    );
    push_quad(
        verts,
        inds,
        [
            vec3(mx.x, mn.y, mn.z),
            vec3(mx.x, mn.y, mx.z),
            vec3(mx.x, mx.y, mx.z),
            vec3(mx.x, mx.y, mn.z),
        ],
        shade(c, 0.68),
    );
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
    let wall_mesh = Mesh {
        vertices: verts,
        indices: inds,
        texture: None,
    };

    let mut gv = Vec::new();
    let mut gi = Vec::new();
    push_quad(
        &mut gv,
        &mut gi,
        [
            vec3(-41.0, 0.0, -31.0),
            vec3(41.0, 0.0, -31.0),
            vec3(41.0, 0.0, 31.0),
            vec3(-41.0, 0.0, 31.0),
        ],
        Color::from_rgba(167, 149, 111, 255),
    );
    for sx in [-1.0f32, 1.0] {
        push_quad(
            &mut gv,
            &mut gi,
            [
                vec3(sx * 32.0, 0.015, -21.0),
                vec3(sx * 19.5, 0.015, -21.0),
                vec3(sx * 19.5, 0.015, -7.0),
                vec3(sx * 32.0, 0.015, -7.0),
            ],
            Color::from_rgba(128, 124, 116, 255),
        );
    }
    for site in [SITE_A, SITE_B] {
        push_quad(
            &mut gv,
            &mut gi,
            [
                vec3(site.x - 4.5, 0.02, site.y - 4.5),
                vec3(site.x + 4.5, 0.02, site.y - 4.5),
                vec3(site.x + 4.5, 0.02, site.y + 4.5),
                vec3(site.x - 4.5, 0.02, site.y + 4.5),
            ],
            Color::from_rgba(190, 120, 60, 255),
        );
    }
    push_quad(
        &mut gv,
        &mut gi,
        [
            vec3(-6.0, 0.02, 23.0),
            vec3(6.0, 0.02, 23.0),
            vec3(6.0, 0.02, 29.0),
            vec3(-6.0, 0.02, 29.0),
        ],
        Color::from_rgba(110, 130, 175, 255),
    );
    push_quad(
        &mut gv,
        &mut gi,
        [
            vec3(-6.0, 0.02, -29.0),
            vec3(6.0, 0.02, -29.0),
            vec3(6.0, 0.02, -23.0),
            vec3(-6.0, 0.02, -23.0),
        ],
        Color::from_rgba(180, 140, 90, 255),
    );
    let ground_mesh = Mesh {
        vertices: gv,
        indices: gi,
        texture: None,
    };

    (walls, vec![ground_mesh, wall_mesh])
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
    step_acc: f32,
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
}

const WPNS: [WpnDef; 2] = [
    WpnDef {
        name: "AK-47",
        dmg: 33,
        auto: true,
        cooldown: 0.1,
        mag_size: 30,
        reserve_start: 90,
        reload: 2.4,
        spread: 0.008,
    },
    WpnDef {
        name: "USP",
        dmg: 34,
        auto: false,
        cooldown: 0.16,
        mag_size: 12,
        reserve_start: 36,
        reload: 2.1,
        spread: 0.006,
    },
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
    wpn: [WpnState; 2],
    spread_add: f32,
    bob: f32,
    switch_t: f32,
    muzzle_t: f32,
    defuse_t: f32,
    plant_t: f32,
    step_t: f32,
    spec_i: usize,

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
    vign: f32,
    hitm: f32,
    shake: f32,

    dbg_open: bool,
    dbg_god: bool,
    dbg_noclip: bool,
    dbg_esp: bool,
    dbg_paths: bool,
    dbg_freeze: bool,

    grabbed: bool,
    last_mouse: Vec2,
}

struct DmgEvent {
    target: Ent,
    dmg: i32,
    killer: String,
    killer_team: Team,
}

impl Game {
    fn is_authority(&self) -> bool {
        !matches!(self.role, Role::Client(_))
    }

    fn player_eye(&self) -> Vec3 {
        self.ppos + vec3(0.0, EYE, 0.0)
    }

    fn look_dir(&self) -> Vec3 {
        vec3(
            self.pitch.cos() * -self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * -self.yaw.cos(),
        )
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
        self.killfeed.push((format!("{killer} > {victim}"), c, 5.0));
        if self.killfeed.len() > 6 {
            self.killfeed.remove(0);
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
        for i in 0..2 {
            self.wpn[i] = WpnState {
                mag: WPNS[i].mag_size,
                reserve: WPNS[i].reserve_start,
                reload_t: 0.0,
                cd: 0.0,
            };
        }
        self.spread_add = 0.0;
        self.defuse_t = 0.0;
        self.plant_t = 0.0;
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
        let frozen = !self.local_phase_live();

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
        if is_key_pressed(KeyCode::Key1) && self.cur != 0 {
            self.cur = 0;
            self.switch_t = 0.45;
            self.wpn[0].reload_t = 0.0;
        }
        if is_key_pressed(KeyCode::Key2) && self.cur != 1 {
            self.cur = 1;
            self.switch_t = 0.35;
            self.wpn[1].reload_t = 0.0;
        }
        let c = self.cur;
        if self.wpn[c].cd > 0.0 {
            self.wpn[c].cd -= dt;
        }
        if self.wpn[c].reload_t > 0.0 {
            self.wpn[c].reload_t -= dt;
            if self.wpn[c].reload_t <= 0.0 {
                let need = WPNS[c].mag_size - self.wpn[c].mag;
                let take = need.min(self.wpn[c].reserve);
                self.wpn[c].mag += take;
                self.wpn[c].reserve -= take;
            }
        }
        if is_key_pressed(KeyCode::R)
            && self.wpn[c].reload_t <= 0.0
            && self.wpn[c].mag < WPNS[c].mag_size
            && self.wpn[c].reserve > 0
        {
            self.wpn[c].reload_t = WPNS[c].reload;
            play(&self.snd.reload, 0.5);
        }

        let busy_e = self.local_defusing() || self.local_planting();
        let want_fire = if WPNS[c].auto {
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
            if self.wpn[c].mag > 0 {
                self.fire_weapon();
            } else if is_mouse_button_pressed(MouseButton::Left) {
                play(&self.snd.reload, 0.3);
                if self.wpn[c].reserve > 0 {
                    self.wpn[c].reload_t = WPNS[c].reload;
                }
            }
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
        self.wpn[c].mag -= 1;
        self.wpn[c].cd = WPNS[c].cooldown;
        let spread = WPNS[c].spread + self.spread_add;
        self.muzzle_t = 0.045;
        self.shake = 0.008;

        let eye = self.player_eye();
        let dir = self.look_dir();
        let right = self.flat_right();
        let up = right.cross(dir).normalize();
        let d = (dir + right * gen_range(-spread, spread) + up * gen_range(-spread, spread)).normalize();

        let wall_t = nearest_wall_hit(eye, d, 300.0, &self.walls);
        let mut best_t = wall_t;
        let mut hit: Option<(Ent, bool)> = None;

        let mut test = |pos: Vec3, ent: Ent, best_t: &mut f32, hit: &mut Option<(Ent, bool)>| {
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
        self.tracers.push((muzzle, end, 0.05));
        if c == 0 {
            play(&self.snd.ak, 0.85);
        } else {
            play(&self.snd.pistol, 0.75);
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

        if let Some((ent, headshot)) = hit {
            let dmg = if headshot { WPNS[c].dmg * 4 } else { WPNS[c].dmg };
            self.hitm = 0.12;
            play(&self.snd.hit, 0.45);
            if self.is_authority() {
                self.apply_damage(ent, dmg, "You".to_string(), self.player_team);
            } else if let Role::Client(net) = &self.role {
                let claim = match ent {
                    Ent::Bot(i) => Some(HitClaim::Bot { idx: i as u8, dmg }),
                    Ent::Remote(id) => Some(HitClaim::Player { id, dmg }),
                    Ent::Me => None,
                };
                net.send(C2S::Shot(ShotMsg {
                    from: a3(muzzle),
                    to: a3(end),
                    pistol: c == 1,
                    hit: claim,
                }));
            }
        } else if let Role::Client(net) = &self.role {
            net.send(C2S::Shot(ShotMsg {
                from: a3(muzzle),
                to: a3(end),
                pistol: c == 1,
                hit: None,
            }));
        }
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
                self.php -= dmg;
                self.vign = (self.vign + 0.45).min(1.0);
                self.shake = 0.02;
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
                    self.add_kill(&killer, killer_team, &victim);
                    play(&self.snd.death, dist_vol(self.player_eye(), pos, 0.6));
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
        self.add_kill(&killer, killer_team, &victim);
        play(&self.snd.death, dist_vol(self.player_eye(), pos, 0.6));
        if self.carrier == Some(Ent::Bot(i)) {
            self.carrier = None;
            self.dropped = Some(pos);
        }
    }

    fn player_die(&mut self, killer: &str, killer_team: Team) {
        self.palive = false;
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
            if step.length_squared() > 0.0 {
                b.step_t -= dt;
                if b.step_t <= 0.0 {
                    b.step_t = 0.36;
                    let v = dist_vol(player_eye, b.pos, 0.4);
                    play(&self.snd.step, v);
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
            // smooth render pos
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
                step_acc: 0.0,
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
                    for i in 0..2 {
                        self.wpn[i] = WpnState {
                            mag: WPNS[i].mag_size,
                            reserve: WPNS[i].reserve_start,
                            reload_t: 0.0,
                            cd: 0.0,
                        };
                    }
                    self.defuse_t = 0.0;
                    self.plant_t = 0.0;
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
        let faces: [(Vec3, Vec3, Vec3, f32); 6] = [
            (u, r, f, 1.0),
            (-u, r, f, 0.45),
            (f, r, u, 0.85),
            (-f, r, u, 0.7),
            (r, f, u, 0.62),
            (-r, f, u, 0.62),
        ];
        for (n, a, b, br) in faces {
            let p = [
                center + n + a + b,
                center + n - a + b,
                center + n - a - b,
                center + n + a - b,
            ];
            push_quad(&mut verts, &mut inds, p, shade(c, br));
        }
        draw_mesh(&Mesh {
            vertices: verts,
            indices: inds,
            texture: None,
        });
    }

    fn draw_character(&self, e: &DrawEnt) {
        let body_c = if e.team == Team::T {
            Color::from_rgba(196, 142, 58, 255)
        } else {
            Color::from_rgba(72, 110, 195, 255)
        };
        let skin = Color::from_rgba(214, 178, 138, 255);
        self.draw_obox(
            e.pos + vec3(0.0, 0.015, 0.0),
            e.yaw,
            vec3(0.45, 0.001, 0.45),
            Color::new(0.0, 0.0, 0.0, 0.3),
        );
        self.draw_obox(
            e.pos + vec3(0.0, 0.3, 0.0),
            e.yaw,
            vec3(0.26, 0.3, 0.17),
            shade(body_c, 0.6),
        );
        self.draw_obox(e.pos + vec3(0.0, 1.0, 0.0), e.yaw, vec3(0.33, 0.42, 0.2), body_c);
        self.draw_obox(e.pos + vec3(0.0, 1.58, 0.0), e.yaw, vec3(0.16, 0.17, 0.16), skin);
        let (s, co) = e.yaw.sin_cos();
        let fwd = vec3(-s, 0.0, -co);
        let right = vec3(co, 0.0, -s);
        self.draw_obox(
            e.pos + vec3(0.0, 1.25, 0.0) + fwd * 0.45 + right * 0.12,
            e.yaw,
            vec3(0.045, 0.06, 0.34),
            Color::from_rgba(40, 38, 36, 255),
        );
        if e.carrier {
            self.draw_obox(
                e.pos + vec3(0.0, 1.05, 0.0) - fwd * 0.3,
                e.yaw,
                vec3(0.16, 0.22, 0.08),
                Color::from_rgba(90, 30, 25, 255),
            );
        }
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
                    });
                }
            }
        } else if let Some(snap) = &self.cstate.snap {
            for b in &snap.bots {
                if b.alive {
                    out.push(DrawEnt {
                        pos: v3(b.pos),
                        yaw: b.yaw,
                        team: Team::from_t(b.team_t),
                        carrier: b.carrier,
                        name: String::new(),
                        hp: 100,
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
            self.draw_character(e);
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
        for (a, b, _) in &self.tracers {
            draw_line_3d(*a, *b, Color::new(1.0, 0.85, 0.4, 0.9));
        }
        for (p, _) in &self.flashes {
            draw_sphere(*p, 0.09, None, Color::new(1.0, 0.8, 0.3, 0.9));
        }
        if let Some((p, age)) = self.explosion {
            let r = 1.0 + age * 28.0;
            let a = (1.0 - age / 0.7).max(0.0);
            draw_sphere(p + vec3(0.0, 1.0, 0.0), r, None, Color::new(1.0, 0.6, 0.15, a * 0.8));
            draw_sphere(p + vec3(0.0, 1.0, 0.0), r * 0.6, None, Color::new(1.0, 0.9, 0.5, a));
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
        let eye = self.player_eye();
        let dir = self.look_dir();
        let right = self.flat_right();
        let up = right.cross(dir).normalize();
        let bob = self.bob.sin() * 0.012;
        let reload_dip = if self.wpn[self.cur].reload_t > 0.0 { 0.18 } else { 0.0 };
        let switch_dip = if self.switch_t > 0.0 { self.switch_t * 0.5 } else { 0.0 };
        let kick = if self.muzzle_t > 0.0 { 0.03 } else { 0.0 };
        let base = eye + dir * (0.55 - kick) + right * 0.24 - up * (0.26 + bob + reload_dip + switch_dip);
        let gyaw = self.yaw;
        let dark = Color::from_rgba(45, 42, 40, 255);
        let wood = Color::from_rgba(110, 75, 45, 255);
        if self.cur == 0 {
            self.draw_obox(base + dir * 0.25, gyaw, vec3(0.035, 0.045, 0.36), dark);
            self.draw_obox(base + dir * 0.02 - up * 0.07, gyaw, vec3(0.03, 0.09, 0.05), wood);
            self.draw_obox(base - dir * 0.18, gyaw, vec3(0.035, 0.06, 0.12), wood);
        } else {
            self.draw_obox(base + dir * 0.12, gyaw, vec3(0.03, 0.045, 0.14), dark);
            self.draw_obox(base - dir * 0.02 - up * 0.07, gyaw, vec3(0.028, 0.08, 0.04), dark);
        }
        if self.muzzle_t > 0.0 {
            let mpos = base + dir * (if self.cur == 0 { 0.65 } else { 0.3 });
            draw_sphere(mpos, 0.05, None, Color::new(1.0, 0.85, 0.4, 0.95));
        }
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

        // name tags for human players
        for e in self.collect_draw_ents() {
            if e.name.is_empty() {
                continue;
            }
            if let Some(s) = self.project(cam, e.pos + vec3(0.0, 2.05, 0.0)) {
                let col = if e.team == Team::T {
                    Color::from_rgba(255, 180, 110, 220)
                } else {
                    Color::from_rgba(140, 190, 255, 220)
                };
                let m = measure_text(&e.name, None, 16, 1.0);
                draw_text(&e.name, s.x - m.width / 2.0, s.y, 16.0, col);
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

        if self.palive {
            let ch = Color::from_rgba(80, 255, 80, 230);
            let gap = 4.0 + self.spread_add * 250.0;
            let len = 9.0;
            draw_line(cx - gap - len, cy, cx - gap, cy, 2.0, ch);
            draw_line(cx + gap, cy, cx + gap + len, cy, 2.0, ch);
            draw_line(cx, cy - gap - len, cx, cy - gap, 2.0, ch);
            draw_line(cx, cy + gap, cx, cy + gap + len, 2.0, ch);
        }

        if self.hitm > 0.0 {
            let a = (self.hitm / 0.12).clamp(0.0, 1.0);
            let c = Color::new(1.0, 1.0, 1.0, a);
            for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                draw_line(cx + dx * 6.0, cy + dy * 6.0, cx + dx * 13.0, cy + dy * 13.0, 2.0, c);
            }
        }

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

        draw_text("HP", 26.0, sh - 46.0, 18.0, Color::from_rgba(150, 220, 150, 255));
        draw_text(&format!("{}", self.php), 26.0, sh - 22.0, 40.0, WHITE);
        let w = &self.wpn[self.cur];
        let wname = WPNS[self.cur].name;
        let ammo_str = if w.reload_t > 0.0 {
            "RELOADING".to_string()
        } else {
            format!("{} / {}", w.mag, w.reserve)
        };
        let m = measure_text(&ammo_str, None, 40, 1.0);
        draw_text(&ammo_str, sw - m.width - 26.0, sh - 22.0, 40.0, WHITE);
        let m2 = measure_text(wname, None, 18, 1.0);
        draw_text(wname, sw - m2.width - 26.0, sh - 50.0, 18.0, Color::from_rgba(255, 210, 120, 255));
        if self.i_am_carrier() {
            draw_text("C4", sw - 70.0, sh - 80.0, 24.0, Color::from_rgba(255, 90, 70, 255));
        }

        let (score_ct, score_t, timer, planted, _phase) = self.hud_numbers();
        let mins = (timer.max(0.0) as i32) / 60;
        let secs = (timer.max(0.0) as i32) % 60;
        let tcol = if planted {
            Color::from_rgba(255, 70, 70, 255)
        } else {
            WHITE
        };
        let bar = format!("CT {}      {}:{:02}      {} T", score_ct, mins, secs, score_t);
        let m3 = measure_text(&bar, None, 26, 1.0);
        draw_rectangle(cx - m3.width / 2.0 - 14.0, 10.0, m3.width + 28.0, 34.0, Color::new(0.0, 0.0, 0.0, 0.5));
        draw_text(&bar, cx - m3.width / 2.0, 34.0, 26.0, tcol);

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
            ];
            draw_rectangle(8.0, 46.0, 170.0, 24.0 + items.len() as f32 * 20.0, Color::new(0.0, 0.0, 0.0, 0.6));
            draw_text("DEBUG (F10)", 16.0, 64.0, 16.0, YELLOW);
            for (k, (label, on)) in items.iter().enumerate() {
                let col = if *on { GREEN } else { Color::from_rgba(200, 200, 200, 255) };
                let state = if *on { " ON" } else { "" };
                draw_text(&format!("{label}{state}"), 16.0, 84.0 + k as f32 * 20.0, 15.0, col);
            }
        }

        if !self.grabbed && self.phase != Phase::Menu {
            let m8 = measure_text("PAUSED - click to resume", None, 30, 1.0);
            draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.45));
            draw_text("PAUSED - click to resume", cx - m8.width / 2.0, cy, 30.0, WHITE);
        }
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
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.03, 0.04, 0.06, 0.82));
        let title = "de_micro";
        let mt = measure_text(title, None, 72, 1.0);
        draw_text(title, cx - mt.width / 2.0, sh * 0.22, 72.0, Color::from_rgba(255, 210, 120, 255));
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

        let lines = [
            "WASD move    Mouse aim / shoot    Space jump    Shift walk",
            "1 AK-47    2 pistol    R reload    E plant / defuse    Esc pause",
            "F10 debug    --host to host online    --join <ticket> to join",
        ];
        let mut y = sh * 0.62;
        for l in lines {
            let m = measure_text(l, None, 20, 1.0);
            draw_text(l, cx - m.width / 2.0, y, 20.0, Color::from_rgba(210, 210, 210, 255));
            y += 30.0;
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
            for i in 0..2 {
                self.wpn[i].mag = WPNS[i].mag_size;
                self.wpn[i].reserve = WPNS[i].reserve_start;
            }
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
        wpn: [WpnState {
            mag: 30,
            reserve: 90,
            reload_t: 0.0,
            cd: 0.0,
        }; 2],
        spread_add: 0.0,
        bob: 0.0,
        switch_t: 0.0,
        muzzle_t: 0.0,
        defuse_t: 0.0,
        plant_t: 0.0,
        step_t: 0.0,
        spec_i: 0,
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
        vign: 0.0,
        hitm: 0.0,
        shake: 0.0,
        dbg_open: false,
        dbg_god: false,
        dbg_noclip: false,
        dbg_esp: false,
        dbg_paths: false,
        dbg_freeze: false,
        grabbed: false,
        last_mouse: mouse_position().into(),
    };

    let mut fps_log = 0u32;

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

        if is_key_pressed(KeyCode::Escape) && g.grabbed {
            g.grabbed = false;
            set_cursor_grab(false);
            show_mouse(true);
        }

        if g.phase == Phase::Menu {
            if is_key_pressed(KeyCode::Key1) {
                g.player_team = Team::Ct;
            }
            if is_key_pressed(KeyCode::Key2) {
                g.player_team = Team::T;
            }
            if g.is_authority() {
                if is_key_pressed(KeyCode::F) {
                    g.ff = !g.ff;
                }
                if is_key_pressed(KeyCode::Up) {
                    g.bot_fill = (g.bot_fill + 1).min(4);
                }
                if is_key_pressed(KeyCode::Down) {
                    g.bot_fill = (g.bot_fill - 1).max(0);
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
        if g.grabbed && g.palive && g.phase != Phase::Menu {
            g.yaw -= mdelta.x * 0.0028;
            g.pitch = (g.pitch - mdelta.y * 0.0028).clamp(-1.45, 1.45);
        }

        g.debug_input();

        let paused = !g.grabbed && g.phase != Phase::Menu && matches!(g.role, Role::Solo);

        if !paused && g.phase != Phase::Menu {
            if g.is_authority() {
                match g.phase {
                    Phase::Freeze => {
                        g.phase_t -= dt;
                        g.remotes_update(dt);
                        if g.phase_t <= 0.0 {
                            g.phase = Phase::Live;
                            g.show_msg("GO!", 1.0);
                            play(&g.snd.beep, 0.4);
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
        } else if g.phase == Phase::Menu && matches!(g.role, Role::Host(_)) {
            // host can sit in menu while clients wait; keep broadcasting empty-ish snaps
            g.host_broadcast(dt);
        }

        clear_background(Color::from_rgba(136, 174, 224, 255));
        let shake_off = if g.shake > 0.0 {
            vec3(gen_range(-g.shake, g.shake), gen_range(-g.shake, g.shake), 0.0)
        } else {
            Vec3::ZERO
        };
        let (eye, dir) = if !g.palive && g.phase != Phase::Menu {
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
            fovy: 80.0_f32.to_radians(),
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

        next_frame().await;
    }
}
