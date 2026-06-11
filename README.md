# de_micro

A miniature **AAA-style** tactical FPS in a single Rust file (macroquad). One hand-built
map, full bomb-defuse gameplay, an economy and buy menu, a weapon roster, baked lighting,
procedural audio, bots with real navigation, and drop-in online multiplayer — a one-map,
fully playable "ultra-premium" demo with no external assets (every mesh, sound and texture
is generated in code).

## Run

```sh
cargo run --release
```

## Feature tour

- **Atmosphere** — a gradient sky dome with a warm sun, baked directional + hemispheric
  lighting with ambient occlusion on every surface, a tiled and weathered concrete floor,
  painted bomb sites, rooftop trim and overhead light fixtures.
- **Weapons** — Knife, USP-S, Glock, Deagle, MP5, AK-47, M4A1-S and a scoped **AWP**
  (right-click to zoom). Each has its own damage, fire rate, recoil pattern, reload, sound
  and view model. Sprays climb with a recovering recoil punch.
- **Economy & buy menu** — earn money for kills and round results, then press **B** during
  freeze time to buy armor + helmet, a defuse kit (CT), or upgrade your primary/secondary.
- **Game feel** — bullet tracers, muzzle flashes, sparks and dust on impacts, persistent
  bullet decals, blood and blood pools, ejected shell casings, floating damage numbers,
  directional damage indicators, hit markers (red on a kill), corpses that linger, screen
  shake, and an explosion shockwave.
- **HUD** — rotating radar with site/bomb/teammate blips, a styled health/armor/money panel,
  weapon + ammo + slot readout, a teammate health overlay, a scoreboard on **Tab**, and an
  animated main menu that orbits the live arena behind it.
- **AI** — bots navigate with a real grid path-finder, take sites, plant and defuse, use
  vision cones, react with delay, burst-fire, strafe, and call out gunfire.
- **Audio** — fully synthesized: layered weapon reports per gun, surface footsteps, an
  ambient wind bed, UI clicks, bomb beeps, a round-start whistle and an explosion.

## Multiplayer (p2p over the internet, no accounts, no port forwarding)

Connectivity uses [iroh](https://iroh.computer) - NAT hole-punching via their free public relays.

**Host** (your machine runs the game world):

```sh
./target/release/de-micro --host
```

A join ticket is printed to the terminal and copied to your clipboard. Send it to friends.

**Join**:

```sh
./target/release/de-micro --join <ticket>            # join as CT
./target/release/de-micro --join <ticket> --team-t   # join as T
./target/release/de-micro --join <ticket> --name Bob # custom name
```

Friends and bots share teams; bots top up each team to the configured fill. Host arbitrates
everything (hits, bomb, rounds); clients are trusted, so play with friends, not strangers.

## How to play

- Menu: 1/2 picks team, F toggles friendly fire, Up/Down sets bot fill (0-4 per team), click to start.
- Ts plant the C4 at site A or B (hold E in a site); CTs kill them or defuse (hold E at the bomb).
- First team to 8 rounds. Dead? LMB/Space cycles spectate.

## Controls

| Key | Action |
|-----|--------|
| WASD | move |
| Mouse | aim / shoot |
| Right mouse | scope (AWP) |
| Space | jump |
| Shift | walk (silent) |
| 1 / 2 / 3 | primary / pistol / knife |
| R | reload |
| B | buy menu (freeze time) |
| E | plant / defuse |
| Tab | scoreboard |
| Esc | pause / release mouse |

## Debug menu (F10)

F1 god, F2 noclip, F3 wallhack ESP, F4 bot paths/vision, F5 freeze AI, F6 kill enemies,
F7 heal + armor + ammo, F8 uncap fps.
