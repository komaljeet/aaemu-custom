//! boss_respawn — 30-minute world boss respawn with killing-blow loot.

use rand::Rng;
use sqlx::{MySqlPool, Row};
use tracing::{info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::{gold_scaling, world_bank};

/// Record a boss kill, schedule the next spawn, and distribute loot to the
/// killing raid.
pub async fn on_boss_killed(
    pool: &MySqlPool,
    cfg: &Config,
    boss_id: i64,
    killing_raid_id: i64,
) -> Result<()> {
    let now = chrono::Utc::now().naive_utc();
    let next = now + chrono::Duration::seconds(cfg.boss.respawn_duration_seconds);
    sqlx::query(
        "INSERT INTO boss_spawn_state (boss_id, last_killed_at, next_spawn_at, killed_by_raid_id) \
         VALUES (?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE last_killed_at = VALUES(last_killed_at), \
         next_spawn_at = VALUES(next_spawn_at), killed_by_raid_id = VALUES(killed_by_raid_id)",
    )
    .bind(boss_id)
    .bind(now)
    .bind(next)
    .bind(killing_raid_id)
    .execute(pool)
    .await?;
    info!(boss_id, raid_id = killing_raid_id, next = ?next, "boss killed — respawn scheduled");
    distribute_boss_loot(pool, cfg, boss_id, killing_raid_id).await
}

/// Distribute personal loot to every member of the killing raid:
/// - gold from the world pool scaled by each member's labor multiplier
/// - a 20–30% thunderstruck chance per member
pub async fn distribute_boss_loot(
    pool: &MySqlPool,
    cfg: &Config,
    boss_id: i64,
    raid_id: i64,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT character_id, account_id FROM raid_members WHERE raid_id = ?",
    )
    .bind(raid_id)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        warn!(boss_id, raid_id, "distribute_boss_loot: raid has no members on record");
        return Ok(());
    }

    let mut rng = rand::rng();
    for r in rows {
        let character_id: i64 = r.try_get("character_id")?;
        let account_id: i64 = r.try_get("account_id")?;

        let multiplier = gold_scaling::get_multiplier(pool, cfg, account_id).await?;
        let gold =
            ((cfg.boss.gold_base_per_member as f64) * multiplier).round() as i64;

        let mut paid = 0i64;
        if gold > 0 && world_bank::can_mint_gold(pool, gold).await? {
            world_bank::log_transaction(pool, account_id, Some(character_id), "reward", gold)
                .await?;
            paid = gold;
        } else if gold > 0 {
            warn!(boss_id, character_id, gold, "boss gold payout skipped — pool empty");
        }

        let threshold: f64 =
            rng.random_range(cfg.boss.thunderstruck_chance_min..=cfg.boss.thunderstruck_chance_max);
        let roll: f64 = rng.random();
        let thunderstruck = roll < threshold;

        sqlx::query(
            "INSERT INTO boss_loot_log (boss_id, raid_id, character_id, gold, thunderstruck) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(boss_id)
        .bind(raid_id)
        .bind(character_id)
        .bind(paid)
        .bind(thunderstruck)
        .execute(pool)
        .await?;

        info!(boss_id, character_id, paid, thunderstruck, "boss loot distributed");
    }
    Ok(())
}

/// Bosses whose `next_spawn_at` has passed.
pub async fn get_bosses_ready_to_spawn(pool: &MySqlPool) -> Result<Vec<i64>> {
    let now = chrono::Utc::now().naive_utc();
    let rows = sqlx::query("SELECT boss_id FROM boss_spawn_state WHERE next_spawn_at <= ?")
        .bind(now)
        .fetch_all(pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(r.try_get("boss_id")?);
    }
    Ok(out)
}

/// Minute scheduler tick — trigger spawns for ready bosses.
///
/// TODO: the actual in-game spawn is performed by the game server; this only
/// clears the timer and logs. Wire a spawn signal once the integration is built.
pub async fn tick_boss_spawns(pool: &MySqlPool) -> Result<usize> {
    let ready = get_bosses_ready_to_spawn(pool).await?;
    for boss_id in &ready {
        info!(boss_id, "tick_boss_spawns: spawn ready — signalling game server");
        sqlx::query("UPDATE boss_spawn_state SET next_spawn_at = NULL WHERE boss_id = ?")
            .bind(boss_id)
            .execute(pool)
            .await?;
    }
    Ok(ready.len())
}

#[cfg(test)]
mod tests {
    // Boss logic is DB + RNG bound; the spawn-ready filter is a simple
    // `next_spawn_at <= now` comparison covered by integration tests.
    // get_mount_speed / get_vehicle_speed math is tested in vehicle_mount_system.
}