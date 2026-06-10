# de_micro

Minimal native Counter-Strike clone in Rust (macroquad). One map, defuse mode, bots.

## Run

```sh
cargo run --release
```

or after building: `./target/release/de-micro`

## How to play

- Pick your team on the menu: press 1 (CT) or 2 (T), then click.
- 4v4 with bots filling both teams. Ts plant the C4 at site A or B; CTs stop them or defuse.
- As T with the bomb: hold E inside a site to plant. As CT: hold E near the bomb to defuse (5s).
- First team to 8 rounds wins the match. When dead, spectate teammates (LMB / Space cycles).

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

| Key | Toggle |
|-----|--------|
| F1 | god mode |
| F2 | noclip fly (Space up, Ctrl down) |
| F3 | wallhack ESP (boxes + hp through walls) |
| F4 | show bot paths + vision lines |
| F5 | freeze bot AI |
| F6 | kill all enemies |
| F7 | full heal + ammo |
