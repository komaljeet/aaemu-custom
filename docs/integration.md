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
| GET  | `/labor/:account_id` | – | `{account_id, pool}` |

### gold_scaling
| Method | Path | Body | Returns |
|--------|------|------|---------|
| GET  | `/gold/multiplier/:account_id` | – | `{account_id, multiplier}` |
| GET  | `/gold/fish/:account_id` | – | `{account_id, gold}` |
| GET  | `/gold/tradepack/:account_id` | – | `{account_id, gold, gilda}` |
| POST | `/gold/coinpurse` | `{account_id, base_gold}` | `{account_id, gold}` |

All rewards return `gold: 0` when the world pool can't mint (rewards pause).

### starter_perks
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/perks/grant` | `{character_id, account_id}` | `{character_id, granted}` |
| POST | `/perks/flight/:mount_id` | – | `{mount_id, updated}` |

### boss_respawn
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/boss/kill` | `{boss_id, raid_id}` | `{boss_id, raid_id, loot}` |
| GET  | `/boss/ready` | – | `{bosses:[ids]}` |

### honor
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/honor/event` | `{account_id, base_honor}` | `{account_id, honor}` |
| POST | `/honor/tome` | `{account_id, character_id}` | `{character_id, skill_points}` |
| GET  | `/honor/price/:item_id` | – | `{item_id, price}` |

### combat_normalization
| Method | Path | Body | Returns |
|--------|------|------|---------|
| POST | `/combat/damage` | `{attacker_id, base_skill_damage}` | `{attacker_id, damage}` |
| POST | `/combat/damage-taken` | `{defender_id, incoming_damage}` | `{defender_id, damage_taken}` |
| GET  | `/combat/stats/:character_id` | – | `{character_id, attack_power, defense_power}` |

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
| World boss killed | NPC death handler (world boss filter) | `OnBossKilledAsync(bossId, raidId)` | pending |
| Honor event reward | honor grant path | `GrantEventHonorAsync(accountId, baseHonor)` | pending |
| Skill point tome used | item use handler | `UseSkillPointTomeAsync(accountId, charId)` | pending |
| Honor shop purchase | shop purchase handler | `GetHonorShopPriceAsync(itemId)` | pending |
| Skill damage dealt | skill effect damage calculation | `CalculateDamageAsync(attackerId, base)` | pending |
| Damage received | defense/damage-taken calculation | `CalculateDamageTakenAsync(defenderId, incoming)` | pending |
| Mount speed queried | mount speed calc | `GetMountSpeedAsync(mountId, buffs)` | pending |
| Vehicle speed queried | vehicle speed calc | `GetVehicleSpeedAsync(vehicleId, buffs)` | pending |
| Labor spent | labor spend calls | `SpendLaborAsync(accountId, amount)` | pending |
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

## Scheduler (sidecar-internal)

The sidecar binary also runs its own background loops — no C# calls needed for
these:

- **hourly** integrity check on the closed-loop invariant
- **per-minute** boss spawn tick (clears ready bosses)
- **per regen interval** labor regen across all accounts
- **daily** wealth tax run (at the hour from `tax_run_schedule`)

These keep the economy and timers consistent even if the C# server is idle.