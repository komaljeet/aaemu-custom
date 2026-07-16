# aaemu-custom

Custom game server systems for an ArcheAge 1.2 private server emulator, written
in Rust. This is a **sidecar** to the C# AAEmu server: it owns custom gameplay
systems and reads/writes the same `aaemu_game` MySQL database. The C# server
remains the authoritative game server; aaemu-custom handles the custom rules.

## Systems

| Module | What it does |
|--------|--------------|
| `world_bank` | Closed-loop gold economy. `circulating + pool == TOTAL_GOLD_CAP` always; daily wealth tax flows back into the pool; hourly integrity check; RMT flags. |
| `labor` | Account-wide labor pool, 100/5min regen (online + offline), 50k cap. |
| `gold_scaling` | Labor-scaled gold rewards (x1–x4). Fish, tradepacks, coinpurses (x20 flat). All payouts check `can_mint_gold`. |
| `starter_perks` | Permanent dragon mount + 4-pack blue hauler on character creation; flight capability for winged mounts (TODO: packet research). |
| `boss_respawn` | 30-min world boss respawn; killing-blow raid gets personal loot (thunderstruck + pool-funded gold). |
| `honor` | Event-only honor x10, shop prices /2, custom Skill Point Tome (1k honor → 1 skill point). |
| `combat_normalization` | Universal Attack Power / Defense Power. `dmg = base*(1+AP/1000)`, `taken = dmg*(1-DP/(DP+5000))`. Armor is cosmetic. |
| `vehicle_mount_system` | Carts/haulers 30m/s no fuel/grease; mounts 21m/s; dragons 30m/s. Buffs add on top. |

## Tech stack

`tokio` (async), `sqlx` (MySQL, runtime queries), `serde`, `chrono`, `thiserror`,
`rand`, `tracing`.

## Layout

```
src/
  lib.rs                    re-exports all modules
  config.rs                 config.toml loader
  error.rs                  shared thiserror Error + Result
  main.rs                   binary: --init-db + scheduler loop
  world_bank.rs             closed-loop economy
  labor.rs                  labor pool
  gold_scaling.rs           labor-scaled rewards
  starter_perks.rs          creation perks + flight
  boss_respawn.rs           boss kill / respawn / loot
  honor.rs                  honor + skill tome
  combat_normalization.rs   AP/DP formulas
  vehicle_mount_system.rs   speeds
schema.sql                  all table definitions (idempotent)
config.toml                 tunables
```

## Build

Rust 1.85+ (built & tested with 1.88). No DATABASE_URL needed at build time —
all SQL is runtime-checked.

```sh
cargo build
cargo test
cargo build --release
```

### Build a Docker image (recommended — no local Rust toolchain needed)

A multi-stage `Dockerfile` builds a slim runtime image (rustls, so no OpenSSL;
debian-slim base):

```sh
docker buildx build -t aaemu-custom:latest --load .
```

The image does **not** bake in `config.toml` (it holds DB credentials) — mount
it at run time (see below).

## Run (end-to-end)

The sidecar shares the C# AAEmu server's `aaemu_game` MySQL database. It needs
its own tables applied and a config file pointing at *your* MySQL credentials.

### 1. Configure

`config.toml` is gitignored (it holds DB credentials — this repo is public).
Copy the template and edit the `[database]` connection to match your AAEmu
MySQL user/password:

```sh
cp config.example.toml config.toml
# then edit config.toml -> [database] connection = "mysql://<user>:<pass>@127.0.0.1:3306/aaemu_game"
```

**If running in Docker on Windows/macOS**, the container can't reach the host's
`127.0.0.1` — use `host.docker.internal` for MySQL and bind the API to
`0.0.0.0`. Make a docker-specific config (also gitignored):

```sh
cp config.example.toml config.docker.toml
# edit config.docker.toml:
#   [api]      listen = "0.0.0.0:1281"
#   [database] connection = "mysql://<user>:<pass>@host.docker.internal:3306/aaemu_game"
```

### 2. One-time DB bootstrap

Apply the schema, initialize the world bank to `total_gold_cap`, and seed
vehicle/mount defaults:

```sh
# native binary
./target/release/aaemu-custom --init-db
#   or:  cargo run --release -- --init-db
#   or:  mysql -u root -p aaemu_game < schema.sql

# docker image (mounts config.docker.toml as /app/config.toml)
docker run --rm --add-host=host.docker.internal:host-gateway \
  -v "$PWD/config.docker.toml:/app/config.toml:ro" \
  aaemu-custom:latest --init-db --config config.toml
```

### 3. Run the sidecar

Starts the HTTP API on `127.0.0.1:1281` plus the scheduler loops (hourly
integrity, per-minute boss spawn tick, labor regen, daily tax):

```sh
# native
./target/release/aaemu-custom
#   or:  cargo run --release
#   or:  cargo run -- --config path/to/custom.toml

# docker (publishes 1281 to the host; AAEmu reaches it at 127.0.0.1:1281)
docker run -d --name aaemu-custom --restart unless-stopped \
  -p 1281:1281 --add-host=host.docker.internal:host-gateway \
  -v "$PWD/config.docker.toml:/app/config.toml:ro" \
  aaemu-custom:latest
# logs:  docker logs -f aaemu-custom
# stop:  docker rm -f aaemu-custom
```

### 4. Enable the AAEmu side

Add the `AaemuCustom` block to AAEmu's `Config.Local.json` (gitignored, per-server):

```json
"AaemuCustom": { "Enabled": true, "BaseUrl": "http://127.0.0.1:1281" }
```

With `Enabled: false` (or the sidecar down), every hook in the C# server is a
no-op and native gameplay runs unchanged — so the sidecar can be enabled or
disabled at any time without restarting the economy.

### Caveats

- **Labor shadow is lazy-seeded.** The sidecar learns about an account only when
  AAEmu first notifies a labor *spend*. Until then the account has no row and a
  1× gold multiplier. This is the intended fresh-start behavior.
- **Combat normalization is opt-in per character.** A character must have a row
  in `character_combat_stats` for the AP/DP model to apply; otherwise that hit
  falls back to AAEmu's native damage calc. NPCs are never seeded, so PvE always
  uses native damage.
- **Boss respawn is scheduled, not spawned.** The sidecar records a ready-to-spawn
  boss and exposes `/boss/ready`, but AAEmu does not yet poll it to create the
  NPC — killed bosses won't reappear via the sidecar until that wiring lands.

## Closed-loop economy invariant

Gold is never created or destroyed. `circulating + pool == TOTAL_GOLD_CAP`
(1,000,000,000). Every mint moves gold pool→circulating; every tax/spend moves
it circulating→pool. `hourly_integrity_check` verifies this and alerts on any
discrepancy (possible dupe/exploit). Rewards pause via `can_mint_gold` when the
pool is empty.