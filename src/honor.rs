//! honor — event-only honor (x10), halved shop prices, Skill Point Tome.

use sqlx::{MySqlPool, Row};
use tracing::info;

use crate::config::Config;
use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Pure helper (no DB) — unit tested
// ---------------------------------------------------------------------------

/// Final honor for an event reward after applying the multiplier.
pub fn event_honor(base_honor: i64, multiplier: i64) -> i64 {
    base_honor * multiplier
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// Grant event honor to an account (x multiplier) and log it.
/// Returns the multiplied honor granted.
pub async fn grant_event_honor(
    pool: &MySqlPool,
    cfg: &Config,
    account_id: i64,
    base_honor: i64,
) -> Result<i64> {
    let multiplied = event_honor(base_honor, cfg.honor.multiplier);
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT IGNORE INTO account_honor (account_id, honor) VALUES (?, 0)")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE account_honor SET honor = honor + ?, updated_at = NOW(3) WHERE account_id = ?")
        .bind(multiplied)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO honor_events (event_id, event_type, account_id, base_honor, multiplied_honor) \
         VALUES (NULL, 'event', ?, ?, ?)",
    )
    .bind(account_id)
    .bind(base_honor)
    .bind(multiplied)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    info!(account_id, base_honor, multiplied, "event honor granted");
    Ok(multiplied)
}

/// Use a Skill Point Tome: deducts honor and grants a skill point to the
/// character. Errors if the account lacks enough honor.
/// Returns the character's new skill-point total.
pub async fn use_skill_point_tome(
    pool: &MySqlPool,
    cfg: &Config,
    account_id: i64,
    character_id: i64,
) -> Result<i64> {
    let cost = cfg.honor.skill_tome_cost;
    let pts = cfg.honor.skill_tome_skill_points;

    let row = sqlx::query("SELECT honor FROM account_honor WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    let have: i64 = row.map(|r| r.try_get::<i64, _>("honor").unwrap_or(0)).unwrap_or(0);
    if have < cost {
        return Err(Error::InsufficientHonor { have, need: cost });
    }

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE account_honor SET honor = honor - ?, updated_at = NOW(3) WHERE account_id = ?")
        .bind(cost)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO skill_point_tomes (account_id, tomes_used, total_skill_points_gained) \
         VALUES (?, 0, 0) ON DUPLICATE KEY UPDATE tomes_used = tomes_used",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE skill_point_tomes SET tomes_used = tomes_used + 1, \
         total_skill_points_gained = total_skill_points_gained + ? WHERE account_id = ?",
    )
    .bind(pts)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO character_skill_points (character_id, points) VALUES (?, 0) \
         ON DUPLICATE KEY UPDATE points = points",
    )
    .bind(character_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE character_skill_points SET points = points + ? WHERE character_id = ?")
        .bind(pts)
        .bind(character_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let new_points: i64 =
        sqlx::query("SELECT points FROM character_skill_points WHERE character_id = ?")
            .bind(character_id)
            .fetch_one(pool)
            .await?
            .try_get("points")?;
    info!(account_id, character_id, cost, new_points, "skill point tome used");
    Ok(new_points)
}

/// Honor shop price for an item (original price divided by the divisor).
pub async fn get_shop_price(pool: &MySqlPool, cfg: &Config, item_id: i64) -> Result<i64> {
    let row = sqlx::query("SELECT original_price FROM honor_shop_prices WHERE item_id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Err(Error::NotFound(format!("honor_shop_prices item_id={item_id}")));
    };
    let original: i64 = row.try_get("original_price")?;
    let divisor = cfg.honor.shop_price_divisor.max(1);
    Ok(original / divisor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_honor_applies_multiplier() {
        assert_eq!(event_honor(100, 10), 1000);
        assert_eq!(event_honor(0, 10), 0);
        assert_eq!(event_honor(7, 10), 70);
    }
}