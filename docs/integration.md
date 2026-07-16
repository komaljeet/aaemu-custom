# aaemu-custom ↔ AAEmu integration

`aaemu-custom` is a **sidecar**: the C# AAEmu game server stays authoritative
for gameplay, and calls the Rust sidecar's HTTP API at specific gameplay moments
to apply the custom rules. The Rust side owns the custom tables in `aaemu_game`
and the closed-loop economy; the C# side owns the wire protocol and the world.

```
 ArcheAge client ──► AAEmu (C#) ──HTTP/JSON──► aaemu-custom (Rust, :1281)
                          │                              │
                          └──────── shared MySQL (aaemu_game) ────────┘
```

## Current progress & next steps

> This is a handoff doc. The integration is incremental; each hook is wired on the C#
> side only and is best-effort (native fallback when the sidecar is down). Per-hook
> status is tracked in the table under [Hook points in AAEmu (C#)](#hook-points-in-aaemu-c).

**Done (C# → sidecar):**
- Character created → starter perks (`CharacterManager.Create`)
- Gold rewards: fish, tradepack, coinpurse — closed-loop economy, labor-scaled, `can_mint`-checked. **Units:** sidecar works in gold; AAEmu wallet/mail is copper (1g = 10000c). Fish, tradepack-gold, and boss-gold hooks ×10000 on award. Coinpurse is a unit-agnostic scalar (copper in → copper out, no conversion).
- Labor spent → sidecar `total_labor_spent` (drives the gold multiplier) (`AccountManager.UpdateLabor`). Notify-only `/labor/spent` (can't-fail) rather than `/labor/spend` (InsufficientLabor on unseeded accounts).
- World boss killed → `OnBossKilledAsync(bossId, teamId, members)` from `Npc.DoDie` (boss-grade filter via `NpcGradeId` ∈ {BossA, BossB, BossC, BossS}); sidecar schedules respawn, mints/logs bank-funded gold per member, rolls thunderstruck, returns per-member loot; C# mails each member their gold (×10000 → copper). Fire-and-forget via `Task.Run` + `BossLootDelivery` (mail works offline).
- Combat damage → `CalculateDamageAsync` / `CalculateDamageTakenAsync` in `DamageEffect.Apply` (combat normalization, AP/DP formula). Replaces AAEmu's armor reduction for **seeded** defenders; unseeded defenders (and NPCs → all PvE) fall back to native. See the combat section below for the unseeded-signal contract.
- Honor event grant → `GrantEventHonorAsync(accountId, amount)` in `GiveHonorPoint.Execute` (the `GiveHonorPoint` special effect — skill/item/quest honor). Sidecar scales by `honor.multiplier` (×10) and records in `account_honor`; falls back to native `HonorRate` on `-1`. Event path only — PvP honor (`AwardPvpHonor`) and the `ChangeGamePoints` funnel are untouched (the funnel also handles spends).
- Honor shop price → `GetHonorShopPriceAsync(itemId)` in `CSBuyItemsPacket.Read` honor branch. Per item, uses the sidecar's `original_price / shop_price_divisor`; falls back to `template.HonorPrice` on `-1` (sidecar down or item not seeded in `honor_shop_prices`). The computed total still drives the affordability check and the spend.

**Pending hooks (suggested order, highest economy value first):**
1. **Skill Point Tome** → `UseSkillPointTomeAsync(accountId, charId)`. **Not a wire — a custom-feature build.** No native handler/item/`SpecialType` exists, and `CharacterSkills` derives available points from `GetSkillPointsForLevel(level)` only (no bonus-point grant API). Needs: a tome item id / trigger skill, a `BonusSkillPoints` grant mechanism on `CharacterSkills`, and a `SpecialEffectAction` calling the sidecar. Sidecar side is ready (`/honor/tome` deducts 1000 honor, grants 1 skill point into `character_skill_points`). Scope as a separate feature.
2. **Mount / vehicle speed** → `GetMountSpeedAsync` / `GetVehicleSpeedAsync` in the mount/vehicle speed calc.
3. **Boss respawn poll** → `GetBossesReadyToSpawnAsync` polled by a spawn tick that calls `NpcManager.Create` at the recorded boss location. Sidecar already schedules the respawn; AAEmu just needs to spawn ready bosses (needs boss-location tracking + native-spawner coordination).
4. (Optional) **RMT flagging** → `FlagRmtSuspectAsync` at gold-transfer audit points.

**Conventions for adding a hook** (use the done hooks as templates):
- **Value-returning hooks in sync C# hot paths:** block on `.GetAwaiter().GetResult()` and fall back to the native value when the sidecar returns `-1`. Safe — AAEmu has no `SynchronizationContext` and the sidecar is local (<10ms; a down sidecar fails the TCP connection instantly). See `DoodadFuncBuyFish`, `SpecialtyManager.SellSpecialty`, `LootPack.GiveLootPack`.
- **Event-notify hooks (no return value needed):** fire-and-forget `_ = AaemuCustomClient.Instance.XxxAsync(...)`. See `CharacterManager.Create`, `AccountManager.UpdateLabor`.
- Add `using AAEmu.Game.Services.AaemuCustom;` at the call site.
- Update the status column in the hook table below when a hook is wired.
- Rebuild the C# server: `dotnet build` from `AAEmu.Game` (expect 0 errors).
- Rebuild the sidecar (Rust isn't on the host): build a Docker image with `docker buildx build -t aaemu-custom:latest --load .` (multi-stage Dockerfile), then run/bootstrap via the image — see `README.md` "Run (end-to-end)". Quick compile check without an image: `MSYS_NO_PATHCONV=1 docker run --rm -v "E:/ArcheAge/aaemu-custom:/app" -v aaemu-cargo-cache:/usr/local/cargo -v aaemu-target:/app/target -w /app rust:1.88 cargo build --release`.

**Repos & branches:**
- C# server: `komaljeet/AAEmu`, PR target `develop`. Per-server config in `AAEmu.Game/Config.Local.json` (gitignored) — `AaemuCustom.Enabled` + `BaseUrl`.
- Rust sidecar: `komaljeet/aaemu-custom`, PR target `main`. Run with `cargo run --release`; serves the API on `127.0.0.1:1281` plus the scheduler loops (hourly integrity, per-minute boss tick, per-regen-interval labor, daily tax). See `README.md` "Run (end-to-end)" for the full bootstrap (config → `--init-db` → run).
- Shared MySQL `aaemu_game` DB; sidecar tables applied via `schema.sql` or `POST /init-db`.

## Setup

1. Apply the custom tables and bootstrap the economy:
   ```sh
   mysql -u root -p aaemu_game < schema.sql
   # or, while the sidecar is running:
   curl -X POST http://127.0.0.1:1281/init-db
   ```
2. Run the sidecar (serves the API + the scheduler loops):
   ```sh
   cargo run --release
   ```
3. Enable the integration on the C# side in `AAEmu.Game/Config.Local.json`:
   ```json
   "AaemuCustom": { "Enabled": true, "BaseUrl": "http://127.0.0.1:1281" }
   ```
   The C# client (`AAEmu.Game.Services.AaemuCustom.AaemuCustomClient`) is
   best-effort: if `Enabled` is false or the sidecar is unreachable, every call
   is a no-op that logs a warning and returns a default. Gameplay is never
   blocked by the sidecar being down.

## HTTP API contract

Base URL default: `http://127.0.0.1:1281`. JSON bodies use snake_case keys.

### world_bank
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/world-bank/integrity` | – | `{invariant:"ok"}` |
| GET  | `/world-bank/health` | – | `{health:"healthy"\|"monitor"\|"warning"\|"critical"}` |
| POST | `/world-bank/tax/run` | – | `{collected}` |
| POST | `/world-bank/mint` | `{account_id, character_id?, amount}` | `{minted}` |
| POST | `/world-bank/log` | `{account_id, character_id?, tx_type, amount}` | `{logged}` |
| POST | `/world-bank/rmt/:account_id` | – | `{account_id, flagged}` |

`tx_type` ∈ `reward`/`mint`/`tax`/`spend`/`transfer_in`/`transfer_out`.

### labor
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/labor/tick/:account_id` | – | `{account_id, pool}` |
| POST | `/labor/spend` | `{account_id, amount}` | `{account_id, pool}` |
| POST | `/labor/spent` | `{account_id, amount}` | `{account_id, total_spent}` |
| GET  | `/labor/:account_id` | – | `{account_id, pool}` |

`/labor/spend` checks the sidecar's own pool and errors `InsufficientLabor` on an
unseeded account. `/labor/spent` is **notify-only**: it creates the row if needed and
just increments `total_labor_spent` (clamping the pool at 0), so it never fails — this
is the one the C# server uses to advance the gold multiplier.

### gold_scaling
| Method | Path | Body | Returns |
|--------|------|------|---------|
| GET  | `/gold/multiplier/:account_id` | – | `{account_id, multiplier}` |
| GET  | `/gold/fish/:account_id` | – | `{account_id, gold}` |
| GET  | `/gold/tradepack/:account_id` | – | `{account_id, gold, gilda}` |
| POST | `/gold/coinpurse` | `{account_id, base_gold}` | `{account_id, gold}` |

All rewards return `gold: 0` when the world pool can't mint (rewards pause).

> **Units:** the sidecar works in **gold** (`fish_base=50`, `gold_base_per_member=100`,
> `total_gold_cap=1_000_000_000`, `rmt_large_transfer_threshold=50000` are all gold).
> AAEmu's wallet (`Money`, `AddMoney`, mail `AttachMoney`) is in **copper** (1g = 10000c;
> see `AddGold.cs: argCopper + argSilver*100 + argGold*10000`). So the C# side must
> **multiply sidecar gold by 10000** before adding to a wallet or attaching to mail.
> Exception: `coinpurse_gold` is a unit-agnostic scalar — the C# side passes the native
> copper coin drop as `base_gold` and awards the result directly as copper (no conversion).

### starter_perks
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/perks/grant` | `{character_id, account_id}` | `{character_id, granted}` |
| POST | `/perks/flight/:mount_id` | – | `{mount_id, updated}` |

### boss_respawn
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/boss/kill` | `{boss_id, raid_id, members:[{character_id, account_id}]}` | `{boss_id, raid_id, loot:[{character_id, gold, thunderstruck}]}` |
| GET  | `/boss/ready` | – | `{bosses:[ids]}` |

`/boss/kill` is called by the C# server on a world-boss death with the killing raid's
roster. The sidecar schedules the respawn (30 min, configurable), mints/logs bank-funded
gold per member (base × labor multiplier, `can_mint`-checked), rolls thunderstruck, and
**returns the per-member loot** so the C# server can deliver the gold in-game (by mail).
If `members` is empty, the sidecar falls back to its `raid_members` table. The returned
`gold` is in **gold units** — the C# server converts to copper (×10000) on delivery.

### honor
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/honor/event` | `{account_id, base_honor}` | `{account_id, honor}` |
| POST | `/honor/tome` | `{account_id, character_id}` | `{character_id, skill_points}` |
| GET  | `/honor/price/:item_id` | – | `{item_id, price}` |

### combat_normalization
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/combat/damage` | `{attacker_id, base_skill_damage}` | `{attacker_id, damage}` — `damage` is `null` when `attacker_id` has no `character_combat_stats` row (unseeded); the C# caller then keeps the unscaled base. |
| POST | `/combat/damage-taken` | `{defender_id, incoming_damage}` | `{defender_id, damage_taken}` — `damage_taken` is `null` when `defender_id` is unseeded; the C# caller falls back to native armor mitigation. |
| GET  | `/combat/stats/:character_id` | – | `{character_id, attack_power, defense_power}` (defaults to `0,0` if unseeded) |

**Unseeded-signal contract:** the damage endpoints return `null` (not `0`) when a
character has no `character_combat_stats` row. This is what lets the C# hook fall
back to native for unseeded characters and NPCs (PvE) instead of stripping their
mitigation. A seeded row with `attack_power=0` / `defense_power=0` is *not* null —
it returns a real value (no AP bonus / full damage), which is the intended
"opted-in but unmodified" state.

### vehicle / mount
| Method | Path | Query | Returns |
|--------|------|-------|---------|
| GET | `/vehicle/speed/:vehicle_id` | `?buffs=` | `{vehicle_id, speed}` |
| GET | `/mount/speed/:mount_id` | `?buffs=` | `{mount_id, speed}` |

### misc
| Method | Path | Returns |
|--------|------|---------|
| GET  | `/health` | `ok` |
| POST | `/init-db` | applies schema + world bank + vehicle/mount defaults |

Error status codes: `404` not found, `400` insufficient labor/honor/pool,
`409` invariant broken / already initialized, `500` other.

## C# client

`AAEmu.Game/Services/AaemuCustom/AaemuCustomClient.cs` — singleton
(`AaemuCustomClient.Instance`), one async method per endpoint, all best-effort.
Add `using AAEmu.Game.Services.AaemuCustom;` at any call site.

## Hook points in AAEmu (C#)

| Gameplay event | C# hook | Client call | Status |
|----------------|---------|-------------|--------|
| Character created | `CharacterManager.Create()` after `SaveDirectlyToDatabase()` | `GrantStarterPerksAsync(charId, accountId)` | ✅ wired |
| Fish caught | `DoodadFuncBuyFish.Use()` (fish turn-in vendor) | `CalculateFishGoldAsync(accountId)` | ✅ wired |
| Tradepack turned in | `SpecialtyManager.SellSpecialty()` before seller mail | `CalculateTradepackRewardAsync(accountId)` | ✅ wired |
| Coinpurse opened | `GainLootPackItemEffect.Apply()` → `LootPack.GiveLootPack(applyCoinpurseScaling:)` | `CalculateCoinpurseGoldAsync(accountId, baseGold)` | ✅ wired |
| World boss killed | `Npc.DoDie()` (boss-grade filter via `NpcGradeId`; roster = `eligiblePlayers`) | `OnBossKilledAsync(bossId, raidId, members)` → mail gold | ✅ wired |
| Honor event reward | `GiveHonorPoint.Execute` (GiveHonorPoint special effect) | `GrantEventHonorAsync(accountId, baseHonor)` | ✅ wired |
| Skill point tome used | item use handler (none yet — feature build) | `UseSkillPointTomeAsync(accountId, charId)` | pending (feature) |
| Honor shop purchase | `CSBuyItemsPacket.Read` honor branch | `GetHonorShopPriceAsync(itemId)` | ✅ wired |
| Skill damage dealt | `DamageEffect.Apply()` (base = pre-reduction `finalDamage`) | `CalculateDamageAsync(attackerId, base)` | ✅ wired |
| Damage received | `DamageEffect.Apply()` (defender DP reduce; replaces native armor for seeded defenders) | `CalculateDamageTakenAsync(defenderId, incoming)` | ✅ wired |
| Mount speed queried | mount speed calc | `GetMountSpeedAsync(mountId, buffs)` | pending |
| Vehicle speed queried | vehicle speed calc | `GetVehicleSpeedAsync(vehicleId, buffs)` | pending |
| Labor spent | `AccountManager.UpdateLabor()` (delta vs. last-known per account) | `RecordLaborSpentAsync(accountId, amount)` | ✅ wired |
| Account flagged for RMT | gold transfer audit | `FlagRmtSuspectAsync(accountId)` | pending |

### Hooking pattern (example)

Character creation is the reference implementation. The pattern for any other
hook is the same — call the client, don't await if the surrounding code is
synchronous, and let the client swallow sidecar failures:

```csharp
// fire-and-forget from a synchronous code path
_ = AaemuCustomClient.Instance.GrantStarterPerksAsync(character.Id, character.AccountId);

// or await from an async path and use the returned value
var gold = await AaemuCustomClient.Instance.CalculateFishGoldAsync(accountId);
if (gold > 0)
    // award `gold` to the player
```

When the sidecar returns a value (e.g. `CalculateDamageAsync`), fall back to the
C# server's native calculation if it returns the failure sentinel (`-1`):

```csharp
var dmg = await AaemuCustomClient.Instance.CalculateDamageAsync(attackerId, baseDamage);
if (dmg < 0)
    dmg = NativeCalculateDamage(attackerId, baseDamage); // AAEmu's own formula
```

### Gold reward hooks (fish / tradepack / coinpurse)

These three hooks are wired into **synchronous** C# hot paths, so they block on the
sidecar call with `.GetAwaiter().GetResult()`. This is safe because AAEmu runs as a
console host with no `SynchronizationContext` (no deadlock risk), the sidecar is local
(replies in <10ms), and a down sidecar fails the TCP connection instantly rather than
waiting on the 5s HTTP timeout. Every hook falls back to AAEmu's native payout when the
sidecar returns its `-1` failure sentinel; a `0` result means the world pool can't mint
and the reward pauses (per spec).

- **Fish** — `DoodadFuncBuyFish.Use()`. The sidecar amount is awarded once via
  `AddMoney` (the canonical wallet path). The native fallback preserves AAEmu's original
  `Money += refund; AddMoney(refund)` lines verbatim.
- **Tradepack** — `SpecialtyManager.SellSpecialty()`, before the seller mail is built.
  Native specialty turn-ins deliver *either* gold *or* gilda depending on the NPC's
  `SpecialtyCoinId` (0 = gold, non-zero = a gilda-style item). The hook maps onto that:
  gold-delivery NPCs pay `sidecar.gold` (100g × labor multiplier); gilda-delivery NPCs
  pay `sidecar.gilda` (10 flat, never scaled). The reward is paid flat to the seller
  (no crafter split) when the sidecar is active. If you want *every* tradepack to pay
  **both** 100g **and** 10 gilda regardless of NPC, that needs a known gilda item
  template id to attach as an extra item — not currently wired.
- **Coinpurse** — detected in `GainLootPackItemEffect.Apply()` by the source item's name
  containing "coinpurse" (Jester's / Prince's / Queen's), then `GiveLootPack` is called
  with `applyCoinpurseScaling: true`. The scaling is applied only on that path, so
  ordinary mob/fishing coin drops through `GiveLootPack` are untouched. `coinCount`
  (the loot pack's native coin drop) is passed as `base_gold` to the sidecar.

### Labor spend hook

The gold multiplier is driven by `total_labor_spent`, so labor spend has to be reported
to the sidecar for the multiplier to move. AAEmu's native labor is **per-account** (the
`Character.LaborPower` doc calls it "Cached representation of Account Labor"; every
change writes `accounts.labor` via `AccountManager.UpdateLabor`), which maps 1:1 onto
the sidecar's per-`account_id` model — no aggregation across characters is needed.

The hook lives in **`AccountManager.UpdateLabor(accountId, laborPower)`** — the single
funnel every labor mutation flows through (skill spend, the `ConsumeLaborPower` special
effect, specialty turn-in, auction-mail copper, the `RecoverExpEffect` direct-setter
bypass, the `TimedRewardsManager` online regen tick, and offline accrual on login). A
`ConcurrentDictionary<accountId, lastLabor>` derives the delta on each call; a **negative
delta** (labor consumed) is fire-and-forget reported via `RecordLaborSpentAsync` →
`POST /labor/spent`. Regen (positive delta) and the first sighting of an account just set
the baseline and are not reported, so login/offline-accrual never falsely counts as spend.
AAEmu stays authoritative for gameplay labor gating; the sidecar only tracks the economy
side. The notify endpoint (`/labor/spent`) is can't-fail by design (creates the row,
clamps the pool at 0) — the authoritative `/labor/spend` is left for a future
sidecar-authoritative-labor mode.

**Existing characters (fresh start):** the sidecar has no historical labor data and AAEmu
has no cumulative-labor-spent stat to back-fill from (`consumed_lp` is persisted but never
incremented in code), so **every account — existing and new — starts at
`total_labor_spent = 0` → 1× gold multiplier** and climbs as labor is spent from now on.
Native gameplay labor (current `accounts.labor`, gating, regen) is untouched.

## Scheduler (sidecar-internal)

The sidecar binary also runs its own background loops — no C# calls needed for
these:

- **hourly** integrity check on the closed-loop invariant
- **per-minute** boss spawn tick (clears ready bosses)
- **per regen interval** labor regen across all accounts
- **daily** wealth tax run (at the hour from `tax_run_schedule`)

These keep the economy and timers consistent even if the C# server is idle.