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
- World boss killed → `OnBossKilledAsync(bossId, raidId, members)` from `Npc.DoDie` (boss-grade filter via `NpcGradeId` ∈ {BossA, BossB, BossC, BossS}; `IsWorldBossGrade` helper). Roster = `eligiblePlayers` ∪ `CharacterTagging.GetAllContributors(MaxLootingRange)` as `(character_id, account_id)`. Sidecar schedules respawn, mints/logs bank-funded gold per member, rolls thunderstruck, returns per-member loot; C# mails each member their gold (×10000 → copper) via `BossLootDelivery` (resolves the name with `NameManager.GetCharacterName` so offline members get mail; `BaseMail` / `MailType.SysExpress` / `AttachMoney`). Fire-and-forget via `Task.Run` (mail works offline; the death thread never blocks on sidecar I/O). **No native gold fallback** — native bosses drop no gold, so a down sidecar (empty loot) just means no payout. `boss_id` is the Npc **template id** (`TemplateId` == `Template.Id`), stable across respawns.
- Combat damage → `CalculateDamageAsync` / `CalculateDamageTakenAsync` in `DamageEffect.Apply` (combat normalization, AP/DP formula). Replaces AAEmu's armor reduction for **seeded** defenders; unseeded defenders (and NPCs → all PvE) fall back to native. See the combat section below for the unseeded-signal contract.
- World boss respawn poll → `BossRespawnPoll.Tick` (registered in `WorldManager.Initialize` on `TickManager` every 5s, `useAsync: true` so sidecar HTTP latency runs on a `Task.Run` worker, not the 20ms tick thread). Polls `GET /boss/ready`; for each ready boss template id, finds the `NpcSpawner`(s) whose `SpawnableNpcs` contain `MemberId == boss_id` (across all worlds via `WorldManager.GetWorlds` + `SpawnManager.GetAllSpawners`) and calls `spawner.DoSpawn()` (the spawner's `WorldSpawnPosition` supplies the original location; `DoSpawn`'s `CurrentSpawnCount >= MaxPopulation` guard prevents double-spawn). Then confirms via `POST /boss/spawned` so the ready signal clears. **Sidecar-authoritative:** on a sidecar-ACKed boss kill, `Npc.DoDie` sets `SidecarManagesRespawn` on the Npc, which makes `NpcSpawner.DoDespawn` skip the native respawn — the sidecar's timer owns re-spawning. Native fallback: if the sidecar is down at kill time (no ACK), the flag stays false and the native respawn proceeds unchanged (and `boss_spawn_state` is never written, so the poll never sees that boss). Self-heals: `boss_spawn_state` persists in MySQL, so if the sidecar dies after ACKing a kill, the poll spawns the boss once the sidecar returns.
- Honor event grant → `GrantEventHonorAsync(accountId, amount)` in `GiveHonorPoint.Execute` (the `GiveHonorPoint` special effect — skill/item/quest honor). Sidecar scales by `honor.multiplier` (×10) and records in `account_honor`; falls back to native `HonorRate` on `-1`. Event path only — PvP honor (`AwardPvpHonor`) and the `ChangeGamePoints` funnel are untouched (the funnel also handles spends).
- Honor shop price → `GetHonorShopPriceAsync(itemId)` in `CSBuyItemsPacket.Read` honor branch. Per item, uses the sidecar's `original_price / shop_price_divisor`; falls back to `template.HonorPrice` on `-1` (sidecar down or item not seeded in `honor_shop_prices`). The computed total still drives the affordability check and the spend.
- Ship base speed → `GetVehicleSpeedAsync(slaveTemplateId, 0)` in `ShipController.ApplyForceAndTorque`. Replaces `shipModel.Velocity` in the forward max-speed cap for ships seeded in `vehicle_stats`; AAEmu's `MoveSpeedMul` + wind physics still apply. **Cached per slave template id** (per-tick hot path): valid results for the process lifetime, failures 30s. Unseeded ships / sidecar down → native. **Mounts and wheeled vehicles are NOT wired** — they're client-authoritative in AAEmu (`CSMoveUnitPacket` only re-broadcasts movement; the server never computes their speed), so a server-side hook can't override their felt speed. The sidecar's flat model (mounts 21, dragons 30, carts 30) is only server-enforceable on ships.
- Skill Point Tome → `UseSkillPointTomeAsync(accountId, charId)` from `CSStartSkillPacket`'s `SkillItem` branch (intercepted by item template id via `SkillPointTome.ItemId` == 19885, repurposed 공간 기록서 / Space Record Book — before the `UseSkillId` check, so the server doesn't care which `use_skill_id` the client uses as a trigger). `SkillPointTome.TryUse` calls the sidecar (`/honor/tome` deducts the configured honor and increments `character_skill_points.points`); on a non-negative `skill_points` total it consumes one tome (`ItemContainer.ConsumeItem` / `ItemTaskType.ConsumeSkillSource`, handles stacks) and syncs `CharacterSkills.BonusSkillPoints` to the returned total; on `-1` (sidecar down/disabled or insufficient honor) it leaves the tome intact and chats an error. `CharacterSkills` adds `BonusSkillPoints` (loaded best-effort from the shared `character_skill_points` table at `Character.Load`) to the available-points calc in `AddSkill`/`AddBuff`, so the server-gated learn allows points beyond the level allowance. The cast bar is resolved instantly with a 0-cast-time `SCSkillStartedPacket` (the tome has no real skill effect; `Write` doesn't serialize the `Skill` object). **Client-side requirement (user):** set item 19885's icon to the red variant and give it a `use_skill_id` (any valid skill) so the client offers "Use" and sends `CSStartSkillPacket`. **Display caveat:** the 1.2 client derives available skill points from level with no server sync packet (`SCCharacterGamePointsPacket` carries only Honor + Vocation), so the bonus total won't show in the skill tree UI — but learning is server-gated, so the extra point is usable once the client sends the learn request.
- Player-to-player gold transfer logging (audit-only, no gating) → `GoldTransfer.LogSend` / `GoldTransfer.LogClaim` from the mail path. `CSSendMailPacket` logs `transfer_out` (sender, attached copper ÷ 10000 → gold) on a successful `MailType.Normal` send; `CharacterMails.GetAttached` logs `transfer_in` (recipient, claimed copper ÷ 10000 → gold) when claiming a `MailType.Normal` mail, then fire-and-forgets `FlagRmtSuspectAsync(recipientAccountId)` so the sidecar evaluates the recipient and writes `rmt_suspects` for admin review. This closes a ledger gap (mail gold previously moved AAEmu's copper wallet without touching `account_gold.balance`) and feeds the sidecar's RMT detector its `transfer_in` data. Both legs are fire-and-forget / best-effort; a down/disabled sidecar is skipped and mail works normally. Sub-gold amounts (< 1g) are skipped (sidecar ledger is whole-gold). Transfers don't touch the `world_bank` tally counters (only `account_gold.balance`, net zero), so the escrow window (debit at send, credit at claim) can never false-trip the hourly integrity check (which only verifies `circulating + pool == cap`). **Scope / limitations:** only `MailType.Normal` player mail is logged on claim — system mails (boss loot `SysExpress`, auction proceeds `AucOffSuccess`) are gameplay rewards already recorded by their own hooks, so they're excluded to avoid double-counting. The mail postage fee is a separate sink not logged here (consistent with other unlogged AAEmu spends). Trade-window gold and COD/auction transfers are not yet wired; Normal mail is the primary vector. **No transfer is ever blocked** — flagging is admin-review-only, so self-transfers (main ↔ alt) always work.

**Pending hooks:**
- None. All economy, labor, combat, honor, boss, skill-point, and transfer-audit hooks are wired. Optional future work: extend transfer logging to the trade window and COD/auction mail types; a sender-side earned-gold gate was considered and deferred (closed-loop supply control + admin visibility suffice for a private server; a gate would add friction for legit new players without stopping established sellers).

**Beyond the hooks (recent work, no new sidecar call sites):**
- Issue #14 — exp / level / stats / party overhaul. **AAEmu-only, no sidecar hook.** See [AAEmu gameplay overrides](#aaemu-gameplay-overrides-no-sidecar-hook).
- Issue #13 — sidecar unit test suite (no runtime effect). See [Testing](#testing).

**Conventions for adding a hook** (use the done hooks as templates):
- **Value-returning hooks in sync C# hot paths:** block on `.GetAwaiter().GetResult()` and fall back to the native value when the sidecar returns `-1`. Safe — AAEmu has no `SynchronizationContext` and the sidecar is local (<10ms; a down sidecar fails the TCP connection instantly). See `DoodadFuncBuyFish`, `SpecialtyManager.SellSpecialty`, `LootPack.GiveLootPack`.
- **Event-notify hooks (no return value needed):** fire-and-forget `_ = AaemuCustomClient.Instance.XxxAsync(...)`. See `CharacterManager.Create`, `AccountManager.UpdateLabor`.
- Add `using AAEmu.Game.Services.AaemuCustom;` at the call site.
- Update the status column in the hook table below when a hook is wired.
- Rebuild the C# server: `dotnet build` from `AAEmu.Game` (expect 0 errors).
- Rebuild the sidecar (Rust isn't on the host): build a Docker image with `docker buildx build -t aaemu-custom:latest --load .` (multi-stage Dockerfile), then run/bootstrap via the image — see `README.md` "Run (end-to-end)". Quick compile check without an image: `MSYS_NO_PATHCONV=1 docker run --rm -v "E:/ArcheAge/aaemu-custom:/app" -v aaemu-cargo-cache:/usr/local/cargo -v aaemu-target:/app/target -w /app rust:1.88 cargo build --release`. Run the no-DB unit tests the same way: swap in `cargo test --lib` (see [Testing](#testing)).

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
| POST | `/boss/spawned` | `{boss_ids:[ids]}` | `{cleared:n}` |

`/boss/kill` is called by the C# server on a world-boss death with the killing raid's
roster. The sidecar schedules the respawn (30 min, configurable), mints/logs bank-funded
gold per member (base × labor multiplier, `can_mint`-checked), rolls thunderstruck, and
**returns the per-member loot** so the C# server can deliver the gold in-game (by mail).
If `members` is empty, the sidecar falls back to its `raid_members` table. The returned
`gold` is in **gold units** — the C# server converts to copper (×10000) on delivery.

`/boss/ready` returns boss template ids whose `next_spawn_at` has elapsed. `/boss/spawned`
clears that ready signal (sets `next_spawn_at = NULL`) for bosses the game server has
(re)spawned, so the respawn poll doesn't re-spawn them next tick. The sidecar's own
per-minute `tick_boss_spawns` is **log-only** — it never clears the ready state (that
would erase the signal before the poll reads it); clearing happens only via `/boss/spawned`.

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
| GET | `/vehicle/speed/:vehicle_id` | `?buffs=` | `{vehicle_id, speed}` — `speed` is `null` when `vehicle_id` has no `vehicle_stats` row (unseeded); the C# caller falls back to the native model speed. |
| GET | `/mount/speed/:mount_id` | `?buffs=` | `{mount_id, speed}` — `null` when unseeded (same contract). **Not currently called from AAEmu** — mounts are client-authoritative (see ship hook notes). |

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
| World boss killed | `Npc.DoDie()` (boss-grade filter via `NpcGradeId`; roster = `eligiblePlayers` ∪ `GetAllContributors`) | `OnBossKilledAsync(bossId, raidId, members)` → `BossLootDelivery` mail gold (×10000→copper) | ✅ wired |
| World boss respawn | `BossRespawnPoll.Tick` (TickManager, every 5s, `useAsync`); spawner lookup by `MemberId == boss_id` → `NpcSpawner.DoSpawn` | `GetBossesReadyToSpawnAsync()` + `MarkBossSpawnedAsync(ids)` | ✅ wired (sidecar-authoritative; native fallback when sidecar down at kill) |
| Honor event reward | `GiveHonorPoint.Execute` (GiveHonorPoint special effect) | `GrantEventHonorAsync(accountId, baseHonor)` | ✅ wired |
| Skill point tome used | `CSStartSkillPacket` SkillItem branch (intercept by item id 19885) → `SkillPointTome.TryUse` | `UseSkillPointTomeAsync(accountId, charId)` | wired |
| Honor shop purchase | `CSBuyItemsPacket.Read` honor branch | `GetHonorShopPriceAsync(itemId)` | ✅ wired |
| Skill damage dealt | `DamageEffect.Apply()` (base = pre-reduction `finalDamage`) | `CalculateDamageAsync(attackerId, base)` | ✅ wired |
| Damage received | `DamageEffect.Apply()` (defender DP reduce; replaces native armor for seeded defenders) | `CalculateDamageTakenAsync(defenderId, incoming)` | ✅ wired |
| Mount speed queried | mount speed calc | `GetMountSpeedAsync(mountId, buffs)` | not server-enforceable (client-authoritative) |
| Vehicle speed (ship) queried | `ShipController.ApplyForceAndTorque` (forward max-speed cap) | `GetVehicleSpeedAsync(slaveTemplateId, 0)` (cached per slave) | ✅ wired |
| Vehicle speed (wheeled) queried | — | `GetVehicleSpeedAsync(vehicleId, buffs)` | not server-enforceable (client-authoritative) |
| Labor spent | `AccountManager.UpdateLabor()` (delta vs. last-known per account) | `RecordLaborSpentAsync(accountId, amount)` | ✅ wired |
| Account flagged for RMT | `CharacterMails.GetAttached` (Normal mail claim) → `GoldTransfer.LogClaim` | `FlagRmtSuspectAsync(accountId)` (audit-only, never blocks) | wired |

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

## Testing

The sidecar has a no-DB unit test suite (`cargo test --lib`) covering every
module's pure calculation helpers — the `*_raw` / pure functions split out
specifically so they can be tested without a live MySQL pool. **44 tests** as of
issue #13. The async `MySqlPool`-bound functions are thin SQL wrappers around
these helpers and are intentionally not covered (an in-memory MySQL shim /
testcontainers for those is a larger follow-up).

Coverage by module:
- `world_bank` — `can_mint_with`, `compute_tax` (incl. empty / custom /
  open-ended tiers, negative balance), `default_tax_tiers` well-formedness,
  `health_from_ratio` boundaries, `check_invariant`, `EconomyHealth::as_str`.
- `gold_scaling` — `multiplier_from_labor` (linear, clamp, negative labor,
  exact inflection), `fish_gold` / `tradepack_gold` / `coinpurse_gold`
  (rounding, zero base).
- `labor` — `regen_amount` (ticks, cap, zero/negative elapsed, zero interval,
  partial-tick floor).
- `combat_normalization` — `compute_damage` / `compute_damage_taken`
  (AP/DP scaling, zero base/incoming, high-DP asymptote).
- `honor` — `event_honor`, `shop_price` (extracted from `get_shop_price`;
  divisor clamped to ≥1 so a misconfigured 0 can't divide-by-zero).
- `boss_respawn` — `loot_gold_per_member`, `next_spawn_at`,
  `thunderstruck_from_randoms` (all extracted from the async DB/RNG path so the
  gold-rounding, respawn timing, and thunderstruck-chance math are testable
  independent of the OS RNG and the database).
- `vehicle_mount_system` — `speed` (additive buffs, negative/zero).
- `config` — `config.example.toml` parses into the current `Config` struct with
  one field per section spot-checked (guards struct/field drift on a fresh
  clone; item ids are placeholder `0`, asserted `>= 0`).

Run them (Rust isn't installed on the host — use the same `rust:1.88` image as
the build):

```sh
MSYS_NO_PATHCONV=1 docker run --rm \
  -v "E:/ArcheAge/aaemu-custom:/app" \
  -v aaemu-cargo-registry:/usr/local/cargo/registry \
  -v aaemu-target:/app/target \
  -w /app rust:1.88 cargo test --lib
```

The cached volumes make reruns fast after the first dependency compile.
`starter_perks` has no pure logic to test (pure `INSERT IGNORE` row-granting), so
it has no unit tests by design.

## AAEmu gameplay overrides (no sidecar hook)

Issue #14 overhauled the exp / level / stat / party system entirely on the C#
side — **none of this calls the sidecar** (no hook, no shared table). Recorded
here so the next session doesn't re-derive it or get surprised by level-255
characters. Landed in AAEmu `develop` (PR #9, commit `9c47a070`).

- **No level limit** — `ExperienceManager.PlayerLevelCap` 55 → **255**. The 1.2
  client encodes level as a single byte on the wire (`SCLevelChangedPacket`,
  `SCUnitStatePacket`), so 255 is the hard ceiling — 999 is not reachable without
  a client-protocol change. The shipped `levels` table only has real data to 55
  (a 999,999,056-exp wall at 56, then 1-exp filler to 101) and `TotalExp` is
  `int32`, so the steep stock curve can't extend past ~56.
  `ExperienceManager.Load` keeps DB rows 1-55 and generates a fresh,
  strictly-increasing, int32-safe curve for 56-255 (delta
  `2,000,000 + 50,000 * (level - 56)`; total at 255 ≈ 1.57b). +1 skill point per
  level past 55. Mates keep their stock 50 cap.
- **Stats = level × stats** — the six primary stat getters in `Character`
  (`Str`/`Dex`/`Sta`/`Int`/`Spi`/`Fai`) return `Level * 10` as the base instead
  of the stock `unit_formula` curve; equipment flat adds and buff bonuses still
  apply. Derived stats (`MaxHp`/`MaxMp`/…) auto-scale because their formulas read
  these overridden properties. NPC stat formulas are untouched.
- **×20 exp** — `World.json` `ExpRate` 1.0 → 20.0 (applies to every `AddExp`).
  AAEmu loads `Config.json` → `Configurations/*.json` → `Config.Local.json`
  (last wins); **no `Config.Local.json` exists on disk**, so 20.0 is active.
- **No party/raid exp reduction** — `Npc.DoDie` awards every eligible member (and
  their pet) the full kill XP; `plMod`/`mateMod` stay 1.0 regardless of group
  size. Literal × member-count was judged too extreme for 50-man raids.
- **Removed the ±10 level-difference gate** — the zero-XP hard gate and the
  `levDif` scaling (which would go negative once the gate is gone) are removed,
  so any eligible kill awards full XP regardless of the level gap (high-level
  carry enabled).

**No DB migration.** The 56-255 curve is generated in code at startup; stats
recompute from level on the fly; no schema changes. Existing level-55 characters
stay 55 and can now gain XP beyond 55. Rebuild + restart the AAEmu game server
from `develop` to pick it up.