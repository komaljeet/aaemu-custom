//! labor — account-wide labor pool with online + offline regen.

use chrono::NaiveDateTime;
use sqlx::{MySqlPool, Row};
use tracing::info;

use crate::config::Config;
use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Pure helper (no DB) — unit tested
// ---------------------------------------------------------------------------

/// Apply missed regen ticks since `last_tick`. Returns `(new_pool, ticks)`.
/// Regen is capped at `max_pool` and applies whether online or offline.
pub fn regen_amount(
    current_pool: i64,
    last_tick: NaiveDateTime,
    now: NaiveDateTime,
    interval_secs: i64,
    per_tick: i64,
    max_pool: i64,
) -> (i64, i64) {
    if current_pool >= max_pool {
        return (max_pool, 0);
    }
    let elapsed = (now - last_tick).num_seconds().max(0);
    let ticks = if interval_secs > 0 { elapsed / interval_secs } else { 0 };
    let add = (ticks * per_tick).clamp(0, max_pool - current_pool);
    (current_pool + add, ticks)
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// Catch up an account's labor to now, capped at the max pool.
/// Returns the new pool value.
pub async fn tick_labor_regen(pool: &MySqlPool, cfg: &Config, account_id: i64) -> Result<i64> {
    let now = chrono::Utc::now().naive_utc();
    // Ensure the row exists.
    sqlx::query(
        "INSERT IGNORE INTO account_labor (account_id, pool, last_regen_tick, total_labor_spent) \
         VALUES (?, 0, ?, 0)",
    )
    .bind(account_id)
    .bind(now)
    .execute(pool)
    .await?;

    let row = sqlx::query("SELECT pool, last_regen_tick FROM account_labor WHERE account_id = ?")
        .bind(account_id)
        .fetch_one(pool)
        .await?;
    let current: i64 = row.try_get("pool")?;
    let last: NaiveDateTime = row.try_get("last_regen_tick")?;

    let (new_pool, ticks) = regen_amount(
        current,
        last,
        now,
        cfg.labor.regen_interval_seconds,
        cfg.labor.regen_per_tick,
        cfg.labor.max_pool,
    );

    if ticks > 0 {
        sqlx::query(
            "UPDATE account_labor SET pool = ?, last_regen_tick = ? WHERE account_id = ?",
        )
        .bind(new_pool)
        .bind(now)
        .bind(account_id)
        .execute(pool)
        .await?;
        info!(account_id, new_pool, ticks, "labor regenerated");
    } else {
        // Still advance the tick so we don't recompute the same gap forever.
        sqlx::query("UPDATE account_labor SET last_regen_tick = ? WHERE account_id = ?")
            .bind(now)
            .bind(account_id)
            .execute(pool)
            .await?;
    }
    Ok(new_pool)
}

/// Spend `amount` labor. Errors if the pool can't cover it.
pub async fn spend_labor(pool: &MySqlPool, account_id: i64, amount: i64) -> Result<i64> {
    if amount <= 0 {
        return Err(Error::Other("spend_labor amount must be positive".into()));
    }
    let row = sqlx::query("SELECT pool FROM account_labor WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    let have: i64 = row.map(|r| r.try_get("pool").unwrap_or(0)).unwrap_or(0);
    if have < amount {
        return Err(Error::InsufficientLabor { have, need: amount });
    }
    let new_pool = have - amount;
    sqlx::query(
        "UPDATE account_labor SET pool = ?, total_labor_spent = total_labor_spent + ? WHERE account_id = ?",
    )
    .bind(new_pool)
    .bind(amount)
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(new_pool)
}

/// Current labor pool for an account (0 if none).
pub async fn get_labor(pool: &MySqlPool, account_id: i64) -> Result<i64> {
    let row = sqlx::query("SELECT pool FROM account_labor WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.try_get::<i64, _>("pool").unwrap_or(0)).unwrap_or(0))
}

/// Record that `amount` labor was spent by an account — notify-only, used by the
/// C# server to advance the gold multiplier (which is driven by `total_labor_spent`).
///
/// Unlike `spend_labor`, this never fails on an unseeded account: it creates the row
/// if needed (pool 0) and just increments `total_labor_spent`, clamping the sidecar's
/// own `pool` at 0 so it can still regen internally. AAEmu remains authoritative for
/// gameplay labor gating; this only tracks the economy side. Returns the new
/// `total_labor_spent`.
pub async fn record_labor_spent(pool: &MySqlPool, account_id: i64, amount: i64) -> Result<i64> {
    if amount <= 0 {
        return Err(Error::Other("record_labor_spent amount must be positive".into()));
    }
    let now = chrono::Utc::now().naive_utc();
    sqlx::query(
        "INSERT IGNORE INTO account_labor (account_id, pool, last_regen_tick, total_labor_spent) \
         VALUES (?, 0, ?, 0)",
    )
    .bind(account_id)
    .bind(now)
    .execute(pool)
    .await?;

    sqlx::query(
        "UPDATE account_labor \
         SET pool = GREATEST(0, pool - ?), total_labor_spent = total_labor_spent + ? \
         WHERE account_id = ?",
    )
    .bind(amount)
    .bind(amount)
    .bind(account_id)
    .execute(pool)
    .await?;

    let row = sqlx::query("SELECT total_labor_spent FROM account_labor WHERE account_id = ?")
        .bind(account_id)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get("total_labor_spent")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn t(min: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            + chrono::Duration::minutes(min)
    }

    #[test]
    fn regen_adds_ticks_and_caps() {
        // 15 minutes = 3 ticks of 5 minutes -> 300 labor.
        let (pool, ticks) = regen_amount(0, t(0), t(15), 300, 100, 50_000);
        assert_eq!(ticks, 3);
        assert_eq!(pool, 300);
    }

    #[test]
    fn regen_caps_at_max() {
        // 1000 minutes would be 200 ticks = 20_000 labor, but cap is 5_000.
        let (pool, _ticks) = regen_amount(4_950, t(0), t(1000), 300, 100, 5_000);
        assert_eq!(pool, 5_000);
    }

    #[test]
    fn regen_no_ticks_when_already_full() {
        let (pool, ticks) = regen_amount(50_000, t(0), t(60), 300, 100, 50_000);
        assert_eq!(pool, 50_000);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn regen_no_rollback_for_zero_elapsed() {
        let (pool, ticks) = regen_amount(100, t(5), t(5), 300, 100, 50_000);
        assert_eq!(pool, 100);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn regen_negative_elapsed_no_rollback() {
        // now before last_tick (clock skew / reordered ticks) -> 0 ticks, pool
        // unchanged. The helper clamps elapsed at 0 rather than regressing.
        let (pool, ticks) = regen_amount(100, t(10), t(0), 300, 100, 50_000);
        assert_eq!(pool, 100);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn regen_zero_interval_no_ticks() {
        // a zero interval is guarded (no divide-by-zero) -> 0 ticks.
        let (pool, ticks) = regen_amount(0, t(0), t(60), 0, 100, 50_000);
        assert_eq!(pool, 0);
        assert_eq!(ticks, 0);
    }

    #[test]
    fn regen_partial_tick_floors() {
        // 7 minutes with a 5-minute interval = 1 tick (floor), not 2.
        let (pool, ticks) = regen_amount(0, t(0), t(7), 300, 100, 50_000);
        assert_eq!(ticks, 1);
        assert_eq!(pool, 100);
    }
}