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

## Run

The binary needs the `aaemu_game` MySQL database (the same one the C# AAEmu
server uses) with the custom tables applied:

```sh
# 1. apply schema + bootstrap world bank + seed defaults
mysql -u root -p aaemu_game < schema.sql
#   or:  aaemu-custom --init-db

# 2. run the scheduler (integrity / boss / labor / daily tax)
cargo run --           # uses config.toml in cwd
cargo run -- --config custom.toml
```

`config.toml` points at `mysql://root:password@127.0.0.1:3306/aaemu_game` —
edit it (or the `[items]` IDs) to match your server.

## Closed-loop economy invariant

Gold is never created or destroyed. `circulating + pool == TOTAL_GOLD_CAP`
(1,000,000,000). Every mint moves gold pool→circulating; every tax/spend moves
it circulating→pool. `hourly_integrity_check` verifies this and alerts on any
discrepancy (possible dupe/exploit). Rewards pause via `can_mint_gold` when the
pool is empty.