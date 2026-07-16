//! starter_perks — permanent perks granted on character creation.

use sqlx::{MySqlPool, Row};
use tracing::{info, warn};

use crate::config::Config;
use crate::error::{Error, Result};

/// Grant the permanent dragon mount and 4-pack blue hauler to a new character.
/// Idempotent — re-running for the same character is a no-op.
/// Returns the number of perks actually granted (0..=2).
pub async fn grant_starter_perks(
    pool: &MySqlPool,
    cfg: &Config,
    character_id: i64,
    account_id: i64,
) -> Result<u64> {
    let dragon = cfg.items.dragon_mount_item_id;
    let hauler = cfg.items.blue_hauler_item_id;

    // Ensure mount metadata exists for both (dragon is true-flight + permanent).
    sqlx::query(
        "INSERT IGNORE INTO mount_metadata (mount_id, has_wings, flight_type, is_permanent) \
         VALUES (?, TRUE, 'TRUE_FLIGHT', TRUE)",
    )
    .bind(dragon)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT IGNORE INTO mount_metadata (mount_id, has_wings, flight_type, is_permanent) \
         VALUES (?, FALSE, 'NONE', TRUE)",
    )
    .bind(hauler)
    .execute(pool)
    .await?;

    let mut granted = 0u64;
    let res = sqlx::query(
        "INSERT IGNORE INTO granted_perks (character_id, account_id, perk_type, item_id, is_permanent) \
         VALUES (?, ?, 'mount', ?, TRUE)",
    )
    .bind(character_id)
    .bind(account_id)
    .bind(dragon)
    .execute(pool)
    .await?;
    granted += res.rows_affected();

    let res = sqlx::query(
        "INSERT IGNORE INTO granted_perks (character_id, account_id, perk_type, item_id, is_permanent) \
         VALUES (?, ?, 'vehicle', ?, TRUE)",
    )
    .bind(character_id)
    .bind(account_id)
    .bind(hauler)
    .execute(pool)
    .await?;
    granted += res.rows_affected();

    info!(character_id, account_id, granted, "starter perks granted (dragon + blue hauler)");
    Ok(granted)
}

/// Grant true-flight to a non-dragon winged mount.
///
/// TODO: pending packet research — flipping `flight_type` here records intent
/// in `mount_metadata`, but the in-game flight capability still needs the
/// correct client packet sequence implemented in the game server.
pub async fn grant_flight_capability(pool: &MySqlPool, mount_id: i64) -> Result<bool> {
    let row = sqlx::query("SELECT has_wings, flight_type FROM mount_metadata WHERE mount_id = ?")
        .bind(mount_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Err(Error::NotFound(format!("mount_metadata mount_id={mount_id}")));
    };
    let has_wings: bool = row.try_get("has_wings")?;
    let flight_type: String = row.try_get("flight_type")?;

    if !has_wings {
        info!(mount_id, "grant_flight_capability: mount has no wings, skipping");
        return Ok(false);
    }
    if flight_type == "TRUE_FLIGHT" {
        return Ok(false); // already true-flight (e.g. dragon)
    }

    warn!(mount_id, "grant_flight_capability: TODO — packet research pending before in-game flight works");
    sqlx::query("UPDATE mount_metadata SET flight_type = 'TRUE_FLIGHT' WHERE mount_id = ?")
        .bind(mount_id)
        .execute(pool)
        .await?;
    info!(mount_id, "grant_flight_capability: flagged TRUE_FLIGHT in mount_metadata");
    Ok(true)
}