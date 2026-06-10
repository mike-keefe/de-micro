use macroquad::audio::{load_sound_from_bytes, play_sound, PlaySoundParams, Sound};
use macroquad::camera::Camera;
use macroquad::models::{draw_mesh, Mesh, Vertex};
use macroquad::prelude::*;
use macroquad::rand::gen_range;

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

const NAMES: [&str; 7] = ["Phoenix", "Viper", "Sandman", "Crusher", "Steel", "Hawk", "Reaper"];

#[derive(Clone, Copy, PartialEq)]
enum Team {
    Ct,
    T,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Menu,
    Freeze,
    Live,
    Post,
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
    // bottom face skipped for static map (never visible), kept for oriented boxes elsewhere
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

    // perimeter
    add(&mut colored, &mut walls, aabb(0.0, -30.5, 82.0, 1.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(0.0, 30.5, 82.0, 1.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-40.5, 0.0, 1.0, 62.0, 5.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(40.5, 0.0, 1.0, 62.0, 5.0, 0.0), wall_col);

    // big building blocks forming B tunnel (west) and Long A (east)
    add(&mut colored, &mut walls, aabb(-36.0, -14.0, 8.0, 18.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(-16.0, -14.0, 7.0, 14.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(36.0, -14.0, 8.0, 18.0, 4.5, 0.0), block_col);
    add(&mut colored, &mut walls, aabb(16.0, -14.0, 7.0, 14.0, 4.5, 0.0), block_col);

    // tunnel / long archway lintels
    add(&mut colored, &mut walls, aabb(-25.75, -21.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(-25.75, -7.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(25.75, -21.0, 12.5, 1.0, 2.1, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(25.75, -7.0, 12.5, 1.0, 2.1, 2.4), lintel_col);

    // mid lane dividers with connector gaps to sites
    add(&mut colored, &mut walls, aabb(-12.0, -10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-12.0, 10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(12.0, -10.5, 1.0, 15.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(12.0, 10.5, 1.0, 15.0, 4.0, 0.0), wall_col);

    // mid doors
    add(&mut colored, &mut walls, aabb(-7.25, 0.0, 8.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(7.25, 0.0, 8.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(0.0, 0.0, 6.0, 1.0, 2.0, 2.5), lintel_col);

    // site south walls with CT doors
    add(&mut colored, &mut walls, aabb(-33.0, 14.0, 15.0, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-17.75, 14.0, 10.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(-24.25, 14.0, 2.5, 1.0, 1.6, 2.4), lintel_col);
    add(&mut colored, &mut walls, aabb(33.0, 14.0, 15.0, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(17.75, 14.0, 10.5, 1.0, 4.0, 0.0), wall_col);
    add(&mut colored, &mut walls, aabb(24.25, 14.0, 2.5, 1.0, 1.6, 2.4), lintel_col);

    // sandbag half-walls (jumpable, block bots)
    add(&mut colored, &mut walls, aabb(-29.0, 3.0, 4.0, 1.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(-21.5, -2.0, 1.0, 4.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(28.0, -3.0, 4.0, 1.0, 1.0, 0.0), bag_col);
    add(&mut colored, &mut walls, aabb(21.5, 2.0, 1.0, 4.0, 1.0, 0.0), bag_col);

    // crates
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
    // stacks (xbox in mid, site doubles)
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

    // ground + zone floors
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
    // tunnel + long floors (stone)
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
    spawn: Vec3,
    anchor: Vec2,
    goal: Vec2,
    path: Vec<Vec2>,
    path_i: usize,
    repath: f32,
    roam_t: f32,
    think: f32,
    target: Option<usize>,
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
            spawn,
            anchor: vec2(spawn.x, spawn.z),
            goal: vec2(spawn.x, spawn.z),
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

    fn reset(&mut self) {
        self.pos = self.spawn;
        self.vy = 0.0;
        self.hp = 100;
        self.alive = true;
        self.yaw = if self.team == Team::T { std::f32::consts::PI } else { 0.0 };
        self.path.clear();
        self.path_i = 0;
        self.repath = 0.0;
        self.target = None;
        self.can_see = false;
        self.react = 0.0;
        self.alert_t = 0.0;
        self.planting = 0.0;
        self.defusing = 0.0;
        self.roam_t = 0.0;
    }
}

#[derive(Clone, Copy)]
struct Snap {
    eye: Vec3,
    alive: bool,
    team: Team,
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

struct Game {
    walls: Vec<Aabb>,
    meshes: Vec<Mesh>,
    nav: Nav,
    snd: Sounds,

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
    spec: Option<usize>,

    bots: Vec<Bot>,

    bomb: Option<Bomb>,
    defused: bool,
    carrier: Option<usize>, // entity index: 0 = player, i+1 = bots[i]
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
    target: usize,
    dmg: i32,
    killer: String,
    killer_team: Team,
    shooter_pos: Vec3,
}

impl Game {
    fn player_eye(&self) -> Vec3 {
        self.ppos + vec3(0.0, EYE, 0.0)
    }

    fn ent_eye(&self, idx: usize) -> Vec3 {
        if idx == 0 {
            self.player_eye()
        } else {
            self.bots[idx - 1].eye()
        }
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
    }

    fn setup_teams(&mut self) {
        let (n_t, n_ct) = if self.player_team == Team::T { (3, 4) } else { (4, 3) };
        self.bots.clear();
        let mut name_i = 0;
        let offsets = [-9.0, -4.5, 4.5, 9.0];
        for k in 0..n_t {
            self.bots.push(Bot::new(NAMES[name_i], Team::T, vec3(offsets[k], 0.0, -26.0)));
            name_i += 1;
        }
        for k in 0..n_ct {
            self.bots.push(Bot::new(NAMES[name_i], Team::Ct, vec3(offsets[k], 0.0, 26.0)));
            name_i += 1;
        }
    }

    fn start_round(&mut self) {
        if self.match_over {
            self.score_ct = 0;
            self.score_t = 0;
            self.round = 0;
            self.match_over = false;
        }
        self.round += 1;
        self.ppos = if self.player_team == Team::T {
            vec3(0.0, 0.0, -26.0)
        } else {
            vec3(0.0, 0.0, 26.0)
        };
        self.yaw = if self.player_team == Team::T { std::f32::consts::PI } else { 0.0 };
        self.pvel = Vec3::ZERO;
        self.pitch = 0.0;
        self.php = 100;
        self.palive = true;
        self.spec = None;
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

        for b in &mut self.bots {
            b.reset();
        }
        self.bomb = None;
        self.defused = false;
        self.dropped = None;
        self.explosion = None;
        self.site = if gen_range(0, 2) == 0 { SITE_A } else { SITE_B };

        // bomb to random T entity (player included if T)
        let mut t_ents: Vec<usize> = Vec::new();
        if self.player_team == Team::T {
            t_ents.push(0);
        }
        for (i, b) in self.bots.iter().enumerate() {
            if b.team == Team::T {
                t_ents.push(i + 1);
            }
        }
        self.carrier = Some(t_ents[gen_range(0, t_ents.len() as i32) as usize]);

        let site = self.site;
        let mut ct_count = 0;
        for b in self.bots.iter_mut() {
            match b.team {
                Team::T => {
                    b.goal = site + vec2(gen_range(-4.0, 4.0), gen_range(-4.0, 4.0));
                }
                Team::Ct => {
                    // anchor one bot per site, extras split
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
        if self.carrier == Some(0) {
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

    // ---------- player ----------

    fn player_update(&mut self, dt: f32) {
        if !self.palive {
            return;
        }
        let frozen = self.phase != Phase::Live;

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

            // footsteps
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

        // weapons
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

        let busy_e = self.is_defusing() || self.is_planting();
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

        // T player: pick up dropped bomb
        if self.player_team == Team::T && self.carrier.is_none() {
            if let Some(dp) = self.dropped {
                if self.ppos.distance(dp) < 1.6 {
                    self.carrier = Some(0);
                    self.dropped = None;
                    self.show_sub("Picked up the C4", 3.0);
                }
            }
        }
    }

    fn is_defusing(&self) -> bool {
        if self.player_team != Team::Ct {
            return false;
        }
        if let Some(b) = &self.bomb {
            !self.defused
                && self.palive
                && self.phase == Phase::Live
                && is_key_down(KeyCode::E)
                && self.ppos.distance(b.pos) < 2.4
        } else {
            false
        }
    }

    fn is_planting(&self) -> bool {
        self.player_team == Team::T
            && self.carrier == Some(0)
            && self.palive
            && self.phase == Phase::Live
            && self.bomb.is_none()
            && is_key_down(KeyCode::E)
            && self.near_any_site()
    }

    fn near_any_site(&self) -> bool {
        let p = vec2(self.ppos.x, self.ppos.z);
        p.distance(SITE_A) < 4.5 || p.distance(SITE_B) < 4.5
    }

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
        let mut hit_bot: Option<(usize, bool)> = None;
        for (i, b) in self.bots.iter().enumerate() {
            if !b.alive || b.team == self.player_team {
                continue;
            }
            let head = Aabb {
                min: b.pos + vec3(-0.18, 1.4, -0.18),
                max: b.pos + vec3(0.18, 1.75, 0.18),
            };
            let body = Aabb {
                min: b.pos + vec3(-0.35, 0.0, -0.35),
                max: b.pos + vec3(0.35, 1.4, 0.35),
            };
            if let Some(t) = ray_aabb(eye, d, &head) {
                if t < best_t {
                    best_t = t;
                    hit_bot = Some((i, true));
                }
            }
            if let Some(t) = ray_aabb(eye, d, &body) {
                if t < best_t {
                    best_t = t;
                    hit_bot = Some((i, false));
                }
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

        // gunfire alerts bots nearby
        for b in &mut self.bots {
            if b.alive && b.pos.distance(eye) < 28.0 {
                b.alert_t = b.alert_t.max(3.0);
            }
        }

        if let Some((i, headshot)) = hit_bot {
            let dmg = if headshot { WPNS[c].dmg * 4 } else { WPNS[c].dmg };
            self.hitm = 0.12;
            play(&self.snd.hit, 0.45);
            self.bots[i].hp -= dmg;
            self.bots[i].alert_t = 4.0;
            if self.bots[i].hp <= 0 && self.bots[i].alive {
                self.kill_bot(i, "You".to_string(), self.player_team);
            }
        }
    }

    fn kill_bot(&mut self, i: usize, killer: String, killer_team: Team) {
        self.bots[i].alive = false;
        let victim = self.bots[i].name.to_string();
        let pos = self.bots[i].pos;
        self.add_kill(&killer, killer_team, &victim);
        play(&self.snd.death, dist_vol(self.player_eye(), pos, 0.6));
        if Some(i + 1) == self.carrier {
            self.carrier = None;
            self.dropped = Some(pos);
        }
    }

    // ---------- bots ----------

    fn bots_update(&mut self, dt: f32) {
        let snaps: Vec<Snap> = std::iter::once(Snap {
            eye: self.player_eye(),
            alive: self.palive,
            team: self.player_team,
        })
        .chain(self.bots.iter().map(|b| Snap {
            eye: b.eye(),
            alive: b.alive,
            team: b.team,
        }))
        .collect();

        let mut events: Vec<DmgEvent> = Vec::new();
        let mut shot_positions: Vec<Vec3> = Vec::new();
        let bomb_planted = self.bomb.is_some() && !self.defused;
        let bomb_pos = self.bomb.as_ref().map(|b| b.pos);
        let player_eye = self.player_eye();
        let site = self.site;

        let mut planted_now: Option<Vec3> = None;
        let mut defused_now: Option<&'static str> = None;
        let mut pickup: Option<usize> = None;

        for i in 0..self.bots.len() {
            let b = &mut self.bots[i];
            if !b.alive {
                continue;
            }
            b.alert_t = (b.alert_t - dt).max(0.0);

            // targeting: vision cone + hearing + alert
            b.think -= dt;
            if b.think <= 0.0 {
                b.think = 0.1 + gen_range(0.0, 0.08);
                let my_eye = b.eye();
                let facing = vec3(-b.yaw.sin(), 0.0, -b.yaw.cos());
                let mut best: Option<(usize, f32)> = None;
                for (j, s) in snaps.iter().enumerate() {
                    if j == i + 1 || !s.alive || s.team == b.team {
                        continue;
                    }
                    let to = s.eye - my_eye;
                    let d = to.length();
                    if d > 55.0 {
                        continue;
                    }
                    let already = b.target == Some(j);
                    if !already && b.alert_t <= 0.0 && d > 9.0 {
                        let flat = vec3(to.x, 0.0, to.z).normalize_or_zero();
                        if facing.dot(flat) < 0.57 {
                            continue; // outside ~110 degree cone, didn't hear anything
                        }
                    }
                    if !los_clear(my_eye, s.eye, &self.walls) {
                        continue;
                    }
                    if best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((j, d));
                    }
                }
                match (b.target, best) {
                    (None, Some((j, _))) => {
                        b.target = Some(j);
                        b.react = 0.5 + gen_range(0.0, 0.5);
                        b.can_see = true;
                        b.lost_t = 0.0;
                    }
                    (Some(_), Some((j, _))) => {
                        b.target = Some(j);
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

            let in_combat = b.target.is_some() && b.can_see;

            if in_combat {
                let tgt = b.target.unwrap();
                let tsnap = snaps[tgt];
                let to = tsnap.eye - b.eye();
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
                        let hit = gen_range(0.0, 1.0) < p_hit;
                        let muzzle = b.eye() + to.normalize() * 0.5;
                        let jitter = if hit { 0.12 } else { 1.4 };
                        let aim = tsnap.eye
                            + vec3(
                                gen_range(-jitter, jitter),
                                gen_range(-jitter, jitter),
                                gen_range(-jitter, jitter),
                            );
                        self.tracers.push((muzzle, aim, 0.05));
                        self.flashes.push((muzzle, 0.04));
                        shot_positions.push(muzzle);
                        let vol = dist_vol(player_eye, muzzle, 0.8);
                        if muzzle.distance(player_eye) > 28.0 {
                            play(&self.snd.far, vol);
                        } else {
                            play(&self.snd.ak, vol * 0.8);
                        }
                        if hit {
                            events.push(DmgEvent {
                                target: tgt,
                                dmg: gen_range(14, 26),
                                killer: b.name.to_string(),
                                killer_team: b.team,
                                shooter_pos: muzzle,
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

            // objectives
            let is_carrier = Some(i + 1) == self.carrier;
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
                                        defused_now = Some(b.name);
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

            // movement
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
            // bot footsteps
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

        // gunfire alerts bots near shooters
        for sp in shot_positions {
            for b in &mut self.bots {
                if b.alive && b.pos.distance(sp) < 22.0 {
                    b.alert_t = b.alert_t.max(3.0);
                }
            }
        }

        if let Some(i) = pickup {
            self.carrier = Some(i + 1);
            self.dropped = None;
        }
        if let Some(pos) = planted_now {
            self.plant_bomb(pos);
        }
        if let Some(name) = defused_now {
            self.defused = true;
            play(&self.snd.defused, 0.9);
            self.show_sub(&format!("{name} defused the bomb"), 4.0);
            self.end_round(Team::Ct, "bomb defused");
        }

        // damage application
        for ev in events {
            if self.phase != Phase::Live {
                break;
            }
            if ev.target == 0 {
                if !self.palive || self.dbg_god {
                    continue;
                }
                self.php -= ev.dmg;
                self.vign = (self.vign + 0.45).min(1.0);
                self.shake = 0.02;
                if self.php <= 0 {
                    self.php = 0;
                    self.player_die(&ev.killer, ev.killer_team);
                }
            } else {
                let bi = ev.target - 1;
                if !self.bots[bi].alive || self.bots[bi].team == ev.killer_team {
                    continue;
                }
                self.bots[bi].hp -= ev.dmg;
                self.bots[bi].alert_t = 4.0;
                self.bots[bi].defusing = 0.0;
                self.bots[bi].planting = 0.0;
                let _ = ev.shooter_pos;
                if self.bots[bi].hp <= 0 {
                    self.kill_bot(bi, ev.killer, ev.killer_team);
                }
            }
        }
    }

    fn player_die(&mut self, killer: &str, killer_team: Team) {
        self.palive = false;
        self.add_kill(killer, killer_team, "You");
        play(&self.snd.death, 0.8);
        self.show_sub("You died - LMB / Space to cycle spectate", 4.0);
        if self.carrier == Some(0) {
            self.carrier = None;
            self.dropped = Some(self.ppos);
        }
        self.spec = self.next_spec(None);
    }

    fn next_spec(&self, after: Option<usize>) -> Option<usize> {
        let n = self.bots.len();
        let start = after.map(|i| i + 1).unwrap_or(0);
        for k in 0..n {
            let i = (start + k) % n;
            if self.bots[i].alive && self.bots[i].team == self.player_team {
                return Some(i);
            }
        }
        // no teammates: spectate anyone alive
        for k in 0..n {
            let i = (start + k) % n;
            if self.bots[i].alive {
                return Some(i);
            }
        }
        None
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
    }

    // ---------- bomb / round flow ----------

    fn bomb_update(&mut self, dt: f32) {
        // player defuse
        if self.is_defusing() {
            self.defuse_t += dt;
            if self.defuse_t >= DEFUSE_TIME && !self.defused {
                self.defused = true;
                play(&self.snd.defused, 0.9);
                self.end_round(Team::Ct, "you defused the bomb");
            }
        } else {
            self.defuse_t = 0.0;
        }
        // player plant
        if self.is_planting() {
            self.plant_t += dt;
            if self.plant_t >= PLANT_TIME && self.bomb.is_none() {
                self.plant_bomb(self.ppos);
                self.plant_t = 0.0;
            }
        } else {
            self.plant_t = 0.0;
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
            self.explosion = Some((bp, 0.0));
            play(&self.snd.explosion, 1.0);
            self.shake = 0.3;
            let d = self.ppos.distance(bp);
            if self.palive && d < 22.0 && !self.dbg_god {
                let dmg = ((1.0 - d / 22.0) * 180.0) as i32;
                self.php -= dmg;
                self.vign = 1.0;
                if self.php <= 0 {
                    self.php = 0;
                    self.player_die("C4", Team::T);
                }
            }
            self.bomb = None;
            self.end_round(Team::T, "target destroyed");
        }
    }

    fn check_round_end(&mut self) {
        if self.phase != Phase::Live {
            return;
        }
        let ts_alive = self.bots.iter().any(|b| b.team == Team::T && b.alive)
            || (self.player_team == Team::T && self.palive);
        let cts_alive = self.bots.iter().any(|b| b.team == Team::Ct && b.alive)
            || (self.player_team == Team::Ct && self.palive);
        let planted = self.bomb.is_some() && !self.defused;
        if !cts_alive {
            self.end_round(Team::T, "Counter-Terrorists eliminated");
        } else if !ts_alive && !planted {
            self.end_round(Team::Ct, "Terrorists eliminated");
        } else if self.round_time <= 0.0 && !planted {
            self.end_round(Team::Ct, "time ran out");
        }
    }

    // ---------- debug ----------

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
        if is_key_pressed(KeyCode::F6) {
            let enemy = if self.player_team == Team::Ct { Team::T } else { Team::Ct };
            for i in 0..self.bots.len() {
                if self.bots[i].alive && self.bots[i].team == enemy {
                    self.kill_bot(i, "Debug".to_string(), self.player_team);
                }
            }
        }
        if is_key_pressed(KeyCode::F7) {
            self.php = 100;
            self.palive = true;
            for i in 0..2 {
                self.wpn[i].mag = WPNS[i].mag_size;
                self.wpn[i].reserve = WPNS[i].reserve_start;
            }
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

    fn draw_bot(&self, b: &Bot, idx: usize) {
        let body_c = if b.team == Team::T {
            Color::from_rgba(196, 142, 58, 255)
        } else {
            Color::from_rgba(72, 110, 195, 255)
        };
        let skin = Color::from_rgba(214, 178, 138, 255);
        self.draw_obox(
            b.pos + vec3(0.0, 0.015, 0.0),
            b.yaw,
            vec3(0.45, 0.001, 0.45),
            Color::new(0.0, 0.0, 0.0, 0.3),
        );
        self.draw_obox(
            b.pos + vec3(0.0, 0.3, 0.0),
            b.yaw,
            vec3(0.26, 0.3, 0.17),
            shade(body_c, 0.6),
        );
        self.draw_obox(b.pos + vec3(0.0, 1.0, 0.0), b.yaw, vec3(0.33, 0.42, 0.2), body_c);
        self.draw_obox(b.pos + vec3(0.0, 1.58, 0.0), b.yaw, vec3(0.16, 0.17, 0.16), skin);
        let (s, co) = b.yaw.sin_cos();
        let fwd = vec3(-s, 0.0, -co);
        let right = vec3(co, 0.0, -s);
        self.draw_obox(
            b.pos + vec3(0.0, 1.25, 0.0) + fwd * 0.45 + right * 0.12,
            b.yaw,
            vec3(0.045, 0.06, 0.34),
            Color::from_rgba(40, 38, 36, 255),
        );
        if Some(idx + 1) == self.carrier {
            self.draw_obox(
                b.pos + vec3(0.0, 1.05, 0.0) - fwd * 0.3,
                b.yaw,
                vec3(0.16, 0.22, 0.08),
                Color::from_rgba(90, 30, 25, 255),
            );
        }
    }

    fn draw_world(&self, t_now: f64) {
        for m in &self.meshes {
            draw_mesh(m);
        }
        for (i, b) in self.bots.iter().enumerate() {
            if b.alive {
                self.draw_bot(b, i);
            }
        }
        if let Some(dp) = self.dropped {
            self.draw_obox(
                dp + vec3(0.0, 0.12, 0.0),
                0.0,
                vec3(0.2, 0.12, 0.12),
                Color::from_rgba(120, 35, 30, 255),
            );
        }
        if let Some(b) = &self.bomb {
            self.draw_obox(
                b.pos + vec3(0.0, 0.12, 0.0),
                0.6,
                vec3(0.22, 0.12, 0.14),
                Color::from_rgba(110, 32, 28, 255),
            );
            let blink = ((t_now * (2.0 + (1.0 - (b.t / BOMB_TIME) as f64) * 10.0)).sin() > 0.0) as i32;
            if blink == 1 && !self.defused {
                draw_sphere(b.pos + vec3(0.12, 0.28, 0.0), 0.05, None, RED);
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
        // debug: bot paths + vision lines
        if self.dbg_paths {
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
                        draw_line_3d(b.eye(), self.ent_eye(t), Color::new(1.0, 0.1, 0.1, 0.9));
                    }
                }
            }
        }
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

    fn draw_hud(&self, cam: &Camera3D) {
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
        if let Some(b) = &self.bomb {
            if !self.defused {
                if let Some(s) = self.project(cam, b.pos + vec3(0.0, 0.8, 0.0)) {
                    let m = measure_text("C4", None, 22, 1.0);
                    draw_text("C4", s.x - m.width / 2.0, s.y, 22.0, Color::new(1.0, 0.3, 0.25, 0.9));
                }
            }
        }

        // ESP wallhack
        if self.dbg_esp {
            for b in &self.bots {
                if !b.alive {
                    continue;
                }
                let col = if b.team == Team::T {
                    Color::from_rgba(255, 160, 70, 255)
                } else {
                    Color::from_rgba(110, 170, 255, 255)
                };
                if let (Some(top), Some(bottom)) = (
                    self.project(cam, b.pos + vec3(0.0, 1.85, 0.0)),
                    self.project(cam, b.pos),
                ) {
                    let h = (bottom.y - top.y).abs().max(4.0);
                    let w = h * 0.45;
                    draw_rectangle_lines(top.x - w / 2.0, top.y, w, h, 1.5, col);
                    let label = format!("{} {}hp", b.name, b.hp);
                    let m = measure_text(&label, None, 14, 1.0);
                    draw_text(&label, top.x - m.width / 2.0, top.y - 4.0, 14.0, col);
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
            if let Some(i) = self.spec {
                let label = format!("Spectating {}  (LMB / Space: next)", self.bots[i].name);
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
        if self.carrier == Some(0) {
            draw_text("C4", sw - 70.0, sh - 80.0, 24.0, Color::from_rgba(255, 90, 70, 255));
        }

        let timer = if let Some(b) = &self.bomb {
            if self.defused {
                0.0
            } else {
                b.t
            }
        } else {
            self.round_time
        };
        let mins = (timer.max(0.0) as i32) / 60;
        let secs = (timer.max(0.0) as i32) % 60;
        let tcol = if self.bomb.is_some() && !self.defused {
            Color::from_rgba(255, 70, 70, 255)
        } else {
            WHITE
        };
        let bar = format!("CT {}      {}:{:02}      {} T", self.score_ct, mins, secs, self.score_t);
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

        // E-interaction bars
        let bar_y = sh * 0.68;
        if self.defuse_t > 0.0 {
            self.draw_progress(cx, bar_y, "DEFUSING", self.defuse_t / DEFUSE_TIME);
        } else if self.plant_t > 0.0 {
            self.draw_progress(cx, bar_y, "PLANTING", self.plant_t / PLANT_TIME);
        } else if self.palive && self.phase == Phase::Live {
            if let Some(b) = &self.bomb {
                if self.player_team == Team::Ct && !self.defused && self.ppos.distance(b.pos) < 2.4 {
                    let hint = "Hold E to defuse";
                    let m7 = measure_text(hint, None, 22, 1.0);
                    draw_text(hint, cx - m7.width / 2.0, bar_y, 22.0, WHITE);
                }
            } else if self.player_team == Team::T && self.carrier == Some(0) && self.near_any_site() {
                let hint = "Hold E to plant the bomb";
                let m7 = measure_text(hint, None, 22, 1.0);
                draw_text(hint, cx - m7.width / 2.0, bar_y, 22.0, WHITE);
            }
        }

        draw_text(&format!("{} fps", get_fps()), 10.0, 20.0, 16.0, Color::new(1.0, 1.0, 1.0, 0.5));

        // debug panel
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
            draw_rectangle(8.0, 30.0, 170.0, 24.0 + items.len() as f32 * 20.0, Color::new(0.0, 0.0, 0.0, 0.6));
            draw_text("DEBUG (F10)", 16.0, 48.0, 16.0, YELLOW);
            for (k, (label, on)) in items.iter().enumerate() {
                let col = if *on { GREEN } else { Color::from_rgba(200, 200, 200, 255) };
                let state = if *on { " ON" } else { "" };
                draw_text(&format!("{label}{state}"), 16.0, 68.0 + k as f32 * 20.0, 15.0, col);
            }
        }

        if !self.grabbed && self.phase != Phase::Menu {
            let m8 = measure_text("PAUSED - click to resume", None, 30, 1.0);
            draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.45));
            draw_text("PAUSED - click to resume", cx - m8.width / 2.0, cy, 30.0, WHITE);
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
        draw_text(title, cx - mt.width / 2.0, sh * 0.26, 72.0, Color::from_rgba(255, 210, 120, 255));
        let sub = "Defuse mode vs bots - first to 8 rounds";
        let ms = measure_text(sub, None, 24, 1.0);
        draw_text(sub, cx - ms.width / 2.0, sh * 0.26 + 36.0, 24.0, Color::from_rgba(180, 180, 180, 255));

        let ct_sel = self.player_team == Team::Ct;
        let ct_label = if ct_sel { "> [1] Counter-Terrorists <" } else { "[1] Counter-Terrorists" };
        let t_label = if !ct_sel { "> [2] Terrorists <" } else { "[2] Terrorists" };
        let mc = measure_text(ct_label, None, 28, 1.0);
        draw_text(
            ct_label,
            cx - mc.width / 2.0,
            sh * 0.42,
            28.0,
            if ct_sel { Color::from_rgba(110, 170, 255, 255) } else { GRAY },
        );
        let mt2 = measure_text(t_label, None, 28, 1.0);
        draw_text(
            t_label,
            cx - mt2.width / 2.0,
            sh * 0.42 + 34.0,
            28.0,
            if !ct_sel { Color::from_rgba(255, 160, 70, 255) } else { GRAY },
        );

        let lines = [
            "WASD move    Mouse aim / shoot    Space jump    Shift walk",
            "1 AK-47    2 pistol    R reload    E plant / defuse    Esc pause",
            "F10 debug menu    LMB / Space cycle spectate when dead",
        ];
        let mut y = sh * 0.58;
        for l in lines {
            let m = measure_text(l, None, 20, 1.0);
            draw_text(l, cx - m.width / 2.0, y, 20.0, Color::from_rgba(210, 210, 210, 255));
            y += 30.0;
        }
        let go = "CLICK TO PLAY";
        let mg = measure_text(go, None, 30, 1.0);
        let pulse = ((get_time() * 3.0).sin() * 0.3 + 0.7) as f32;
        draw_text(go, cx - mg.width / 2.0, sh * 0.76, 30.0, Color::new(0.3, 1.0, 0.3, pulse));
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
    let snd = Sounds::load().await;
    let (walls, meshes) = build_map();
    let nav = Nav::build(&walls);

    let mut g = Game {
        walls,
        meshes,
        nav,
        snd,
        player_team: Team::Ct,
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
        spec: None,
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

        if is_key_pressed(KeyCode::Escape) && g.grabbed {
            g.grabbed = false;
            set_cursor_grab(false);
            show_mouse(true);
        }

        // menu team select
        if g.phase == Phase::Menu {
            if is_key_pressed(KeyCode::Key1) {
                g.player_team = Team::Ct;
            }
            if is_key_pressed(KeyCode::Key2) {
                g.player_team = Team::T;
            }
        }

        if is_mouse_button_pressed(MouseButton::Left) && !g.grabbed {
            g.grabbed = true;
            set_cursor_grab(true);
            show_mouse(false);
            g.last_mouse = mouse_position().into();
            if g.phase == Phase::Menu {
                g.setup_teams();
                g.start_round();
            }
        } else if g.grabbed && !g.palive && g.phase != Phase::Menu {
            // spectate cycling
            if is_mouse_button_pressed(MouseButton::Left) || is_key_pressed(KeyCode::Space) {
                g.spec = g.next_spec(g.spec);
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

        let paused = !g.grabbed && g.phase != Phase::Menu;

        if !paused && g.phase != Phase::Menu {
            match g.phase {
                Phase::Freeze => {
                    g.phase_t -= dt;
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
                    if !g.dbg_freeze {
                        g.bots_update(dt);
                    }
                    g.bomb_update(dt);
                    g.check_round_end();
                }
                Phase::Post => {
                    g.phase_t -= dt;
                    g.player_update(dt);
                    g.bomb_update(dt);
                    if g.phase_t <= 0.0 {
                        g.start_round();
                    }
                }
                Phase::Menu => {}
            }

            // dead spectator auto-advance if target died
            if !g.palive {
                if let Some(i) = g.spec {
                    if !g.bots[i].alive {
                        g.spec = g.next_spec(Some(i));
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
        }

        // camera: player, or spectated bot when dead
        clear_background(Color::from_rgba(136, 174, 224, 255));
        let shake_off = if g.shake > 0.0 {
            vec3(gen_range(-g.shake, g.shake), gen_range(-g.shake, g.shake), 0.0)
        } else {
            Vec3::ZERO
        };
        let (eye, dir) = if !g.palive && g.spec.is_some() && g.phase != Phase::Menu {
            let b = &g.bots[g.spec.unwrap()];
            let d = vec3(-b.yaw.sin(), 0.0, -b.yaw.cos());
            (b.eye() + shake_off, d)
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
        g.draw_world(t_now);
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
