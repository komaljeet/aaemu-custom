//! vehicle_mount_system — flat speed model with no fuel/grease.
//!
//! - carts / haulers : 30 m/s base, no fuel, no grease
//! - mounts          : 21 m/s base
//! - dragon mounts   : 30 m/s base
//! - speed buffs add on top of the base speed

use sqlx::{MySqlPool, Row};
use tracing::info;

use crate::config::Config;
use crate::error::Result;

// ---------------------------------------------------------------------------
// Pure helper (no DB) — unit tested
// ---------------------------------------------------------------------------

/// Final speed = base + additive buffs.
pub fn speed(base_speed: f32, speed_buffs: f32) -> f32 {
    base_speed + speed_buffs
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// Speed for a vehicle (base from `vehicle_stats`, default cart speed if unknown).
pub async fn get_vehicle_speed(
    pool: &MySqlPool,
    cfg: &Config,
    vehicle_id: i64,
    speed_buffs: f32,
) -> Result<f32> {
    let row = sqlx::query("SELECT base_speed FROM vehicle_stats WHERE vehicle_id = ?")
        .bind(vehicle_id)
        .fetch_optional(pool)
        .await?;
    let base = row
        .map(|r| r.try_get::<f32, _>("base_speed").unwrap_or(cfg.mounts.cart_speed))
        .unwrap_or(cfg.mounts.cart_speed);
    Ok(speed(base, speed_buffs))
}

/// Speed for a mount (base from `mount_stats`, default mount speed if unknown).
pub async fn get_mount_speed(
    pool: &MySqlPool,
    cfg: &Config,
    mount_id: i64,
    speed_buffs: f32,
) -> Result<f32> {
    let row = sqlx::query("SELECT base_speed FROM mount_stats WHERE mount_id = ?")
        .bind(mount_id)
        .fetch_optional(pool)
        .await?;
    let base = row
        .map(|r| r.try_get::<f32, _>("base_speed").unwrap_or(cfg.mounts.default_speed))
        .unwrap_or(cfg.mounts.default_speed);
    Ok(speed(base, speed_buffs))
}

/// Reset every vehicle to the default cart speed with no fuel/grease requirement.
pub async fn initialize_vehicle_defaults(pool: &MySqlPool, cfg: &Config) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE vehicle_stats SET base_speed = ?, requires_fuel = FALSE, requires_grease = FALSE",
    )
    .bind(cfg.mounts.cart_speed)
    .execute(pool)
    .await?;
    info!(vehicles = res.rows_affected(), speed = cfg.mounts.cart_speed, "vehicle defaults applied");
    Ok(res.rows_affected())
}

/// Reset mounts: non-dragon to default mount speed, dragon type to dragon speed.
pub async fn initialize_mount_defaults(pool: &MySqlPool, cfg: &Config) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE mount_stats SET base_speed = CASE \
            WHEN is_dragon_type THEN ? ELSE ? END",
    )
    .bind(cfg.mounts.dragon_speed)
    .bind(cfg.mounts.default_speed)
    .execute(pool)
    .await?;
    info!(mounts = res.rows_affected(), "mount defaults applied");
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_adds_buffs() {
        assert_eq!(speed(30.0, 0.0), 30.0);
        assert_eq!(speed(30.0, 5.0), 35.0);
        assert_eq!(speed(21.0, 2.5), 23.5);
    }

    #[test]
    fn mount_and_vehicle_speed_use_same_formula() {
        // Mirrors get_mount_speed/get_vehicle_speed math without a DB.
        assert_eq!(speed(21.0, 0.0), 21.0); // mount, no buffs
        assert_eq!(speed(30.0, 0.0), 30.0); // dragon/cart, no buffs
        assert_eq!(speed(21.0, 9.0), 30.0); // mount with +9 buff == dragon base
    }
}