# de_micro

Minimal native Counter-Strike clone in Rust (macroquad). One map, defuse mode, bots, online multiplayer with zero setup.

## Run

```sh
cargo run --release
```

## Multiplayer (p2p over the internet, no accounts, no port forwarding)

Connectivity uses [iroh](https://iroh.computer) - NAT hole-punching via their free public relays. Nobody signs up for anything.

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

Friends and bots share teams; bots top up each team to the configured fill. Host arbitrates everything (hits, bomb, rounds); clients are trusted, so play with friends, not strangers.

## How to play

- Menu: 1/2 picks team, F toggles friendly fire, Up/Down sets bot fill (0-4 per team), click to start.
- Ts plant the C4 at site A or B (hold E in a site); CTs kill them or defuse (hold E at the bomb).
- First team to 8 rounds. Dead? LMB/Space cycles spectate.

## Controls

| Key | Action |
|-----|--------|
| WASD | move |
| Mouse | aim / shoot |
| Space | jump |
| Shift | walk (silent) |
| 1 / 2 | AK-47 / pistol |
| R | reload |
| E | plant / defuse |
| Esc | pause / release mouse |

## Debug menu (F10)

F1 god, F2 noclip, F3 wallhack ESP, F4 bot paths/vision, F5 freeze AI, F6 kill enemies, F7 heal+ammo.
