//! world_bank — closed-loop gold economy.
//!
//! Invariant: `circulating + pool == TOTAL_GOLD_CAP` at all times. Gold is
//! never created or destroyed, only moved between the world pool and account
//! balances. Tax collected flows back into the pool for redistribution.

use chrono::NaiveDateTime;
use sqlx::{MySqlPool, Row};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::{Error, Result};

/// Economy health derived from the circulating/total ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomyHealth {
    Healthy,
    Monitor,
    Warning,
    Critical,
}

impl EconomyHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            EconomyHealth::Healthy => "healthy",
            EconomyHealth::Monitor => "monitor",
            EconomyHealth::Warning => "warning",
            EconomyHealth::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaxTier {
    pub min_gold: i64,
    pub max_gold: Option<i64>,
    /// Percent, e.g. 0.5 for 0.5%.
    pub rate_pct: f64,
}

/// Default tax tiers matching the spec, used for unit tests and as a fallback.
pub fn default_tax_tiers() -> Vec<TaxTier> {
    vec![
        TaxTier { min_gold: 0, max_gold: Some(5_000), rate_pct: 0.0 },
        TaxTier { min_gold: 5_001, max_gold: Some(50_000), rate_pct: 0.5 },
        TaxTier { min_gold: 50_001, max_gold: Some(500_000), rate_pct: 1.0 },
        TaxTier { min_gold: 500_001, max_gold: None, rate_pct: 2.0 },
    ]
}

// ---------------------------------------------------------------------------
// Pure helpers (no DB) — unit tested
// ---------------------------------------------------------------------------

/// True if the world pool holds enough gold to mint `amount`.
pub fn can_mint_with(amount: i64, pool_balance: i64) -> bool {
    amount > 0 && amount <= pool_balance
}

/// Bracketed wealth tax on a taxable balance. Gold in the exempt bracket is
/// not taxed; each higher bracket taxes only the slice of gold inside it.
pub fn compute_tax(taxable_balance: i64, tiers: &[TaxTier]) -> i64 {
    if taxable_balance <= 0 {
        return 0;
    }
    let mut tax = 0.0f64;
    for t in tiers {
        let lo = t.min_gold;
        let hi = t.max_gold.unwrap_or(i64::MAX);
        if taxable_balance > lo {
            let bracket_top = taxable_balance.min(hi);
            let bracket = (bracket_top - lo) as f64;
            tax += bracket * (t.rate_pct / 100.0);
        }
    }
    tax.round() as i64
}

/// Map a circulating/total ratio to an [`EconomyHealth`] bucket.
pub fn health_from_ratio(ratio: f64) -> EconomyHealth {
    if ratio < 0.50 {
        EconomyHealth::Healthy
    } else if ratio < 0.75 {
        EconomyHealth::Monitor
    } else if ratio < 0.90 {
        EconomyHealth::Warning
    } else {
        EconomyHealth::Critical
    }
}

/// Verify the closed-loop invariant: `circulating + pool == cap`.
pub fn check_invariant(circulating: i64, pool: i64, cap: i64) -> bool {
    circulating + pool == cap
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// One-time world bank bootstrap. Idempotent: if a row already exists this is
/// a no-op (it never resets circulating/pool).
pub async fn initialize_world_bank(pool: &MySqlPool, cfg: &Config) -> Result<()> {
    let existing = sqlx::query("SELECT 1 AS c FROM world_bank WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        warn!("world_bank already initialized — skipping");
        return Ok(());
    }
    let cap = cfg.economy.total_gold_cap;
    sqlx::query(
        "INSERT INTO world_bank (id, total_gold, circulating, pool, taxed_today) \
         VALUES (1, ?, 0, ?, 0)",
    )
    .bind(cap)
    .bind(cap)
    .execute(pool)
    .await?;
    info!(cap, "world_bank initialized — all gold starts in the pool");
    Ok(())
}

/// True if the pool can cover minting `amount` (rewards pause when false).
pub async fn can_mint_gold(pool: &MySqlPool, amount: i64) -> Result<bool> {
    let row = sqlx::query("SELECT pool FROM world_bank WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let pool_balance: i64 = row.try_get("pool")?;
    Ok(can_mint_with(amount, pool_balance))
}

/// Load configured tax tiers from the database.
pub async fn load_tax_tiers(pool: &MySqlPool) -> Result<Vec<TaxTier>> {
    let rows = sqlx::query(
        "SELECT min_gold, max_gold, CAST(rate_pct AS CHAR) AS rate_pct_str \
         FROM tax_tiers ORDER BY min_gold",
    )
    .fetch_all(pool)
    .await?;
    let mut tiers = Vec::with_capacity(rows.len());
    for r in rows {
        let min_gold: i64 = r.try_get("min_gold")?;
        let max_gold: Option<i64> = r.try_get("max_gold")?;
        let rate_str: String = r.try_get("rate_pct_str")?;
        let rate_pct: f64 = rate_str
            .parse()
            .map_err(|e| Error::Other(format!("invalid rate_pct '{rate_str}': {e}")))?;
        tiers.push(TaxTier { min_gold, max_gold, rate_pct });
    }
    Ok(tiers)
}

/// Daily wealth tax for one account. Returns the gold collected (0 if none).
/// `gold_activity.spent_24h` is subtracted from the balance first (gold-in-
/// motion exemption). Collected gold flows back into the world pool.
pub async fn calculate_daily_tax(
    pool: &MySqlPool,
    _cfg: &Config,
    account_id: i64,
) -> Result<i64> {
    let balance: i64 = sqlx::query("SELECT balance FROM account_gold WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?
        .map(|r| r.try_get("balance").unwrap_or(0))
        .unwrap_or(0);

    let spent_24h: i64 = sqlx::query("SELECT spent_24h FROM gold_activity WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?
        .map(|r| r.try_get("spent_24h").unwrap_or(0))
        .unwrap_or(0);

    let taxable = (balance - spent_24h).max(0);
    let tiers = load_tax_tiers(pool).await?;
    let tax = compute_tax(taxable, &tiers);
    if tax <= 0 {
        return Ok(0);
    }

    let now = chrono::Utc::now().naive_utc();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE account_gold SET balance = balance - ?, updated_at = ? WHERE account_id = ?")
        .bind(tax)
        .bind(now)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE world_bank SET circulating = circulating - ?, pool = pool + ?, \
         taxed_today = taxed_today + ?, updated_at = ? WHERE id = 1",
    )
    .bind(tax)
    .bind(tax)
    .bind(tax)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO account_gold_log (account_id, character_id, type, amount) VALUES (?, NULL, 'tax', ?)",
    )
    .bind(account_id)
    .bind(-tax)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(tax)
}

/// Run the daily tax across every account and reset 24h activity afterwards.
/// Returns the total gold collected.
pub async fn run_daily_tax(pool: &MySqlPool, cfg: &Config) -> Result<i64> {
    let rows = sqlx::query("SELECT account_id FROM account_gold")
        .fetch_all(pool)
        .await?;
    let mut total: i64 = 0;
    for r in rows {
        let account_id: i64 = r.try_get("account_id")?;
        total += calculate_daily_tax(pool, cfg, account_id).await?;
    }
    reset_gold_activity(pool).await?;
    info!(total, "daily tax run complete — collected gold returned to the pool");
    Ok(total)
}

/// Record a gold movement and keep the closed-loop invariant intact.
///
/// Known `tx_type` values:
/// - `reward` / `mint` : pool → account (mints gold into circulation)
/// - `tax`             : account → pool
/// - `spend`           : account → pool (vendor/etc.), counts as 24h activity
/// - `transfer_in`     : another account → this account (circulating unchanged)
/// - `transfer_out`    : this account → another account (circulating unchanged)
pub async fn log_transaction(
    pool: &MySqlPool,
    account_id: i64,
    character_id: Option<i64>,
    tx_type: &str,
    amount: i64,
) -> Result<()> {
    let now = chrono::Utc::now().naive_utc();
    // Ensure the account row exists.
    sqlx::query(
        "INSERT IGNORE INTO account_gold (account_id, balance) VALUES (?, 0)",
    )
    .bind(account_id)
    .execute(pool)
    .await?;

    match tx_type {
        "reward" | "mint" => {
            if !can_mint_gold(pool, amount).await? {
                return Err(Error::InsufficientPool {
                    requested: amount,
                    available: 0,
                });
            }
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE account_gold SET balance = balance + ?, updated_at = ? WHERE account_id = ?")
                .bind(amount).bind(now).bind(account_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE world_bank SET pool = pool - ?, circulating = circulating + ?, updated_at = ? WHERE id = 1")
                .bind(amount).bind(amount).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        "tax" => {
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE account_gold SET balance = balance - ?, updated_at = ? WHERE account_id = ?")
                .bind(amount).bind(now).bind(account_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE world_bank SET pool = pool + ?, circulating = circulating - ?, taxed_today = taxed_today + ?, updated_at = ? WHERE id = 1")
                .bind(amount).bind(amount).bind(amount).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        "spend" => {
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE account_gold SET balance = balance - ?, updated_at = ? WHERE account_id = ?")
                .bind(amount).bind(now).bind(account_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE world_bank SET pool = pool + ?, circulating = circulating - ?, updated_at = ? WHERE id = 1")
                .bind(amount).bind(amount).bind(now).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO gold_activity (account_id, spent_24h) VALUES (?, ?) ON DUPLICATE KEY UPDATE spent_24h = spent_24h + ?")
                .bind(account_id).bind(amount).bind(amount).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        "transfer_in" | "transfer_out" => {
            let signed = if tx_type == "transfer_in" { amount } else { -amount };
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE account_gold SET balance = balance + ?, updated_at = ? WHERE account_id = ?")
                .bind(signed).bind(now).bind(account_id).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO gold_activity (account_id, spent_24h) VALUES (?, ?) ON DUPLICATE KEY UPDATE spent_24h = spent_24h + ?")
                .bind(account_id).bind(amount).bind(amount).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        _ => {
            // Unknown type: audit only, no balance change.
        }
    }

    sqlx::query(
        "INSERT INTO account_gold_log (account_id, character_id, type, amount) VALUES (?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(character_id)
    .bind(tx_type)
    .bind(amount)
    .execute(pool)
    .await?;

    Ok(())
}

/// Flag an account as an RMT suspect. Conditions:
/// - a transfer larger than the threshold to an account younger than N days, or
/// - more than `burst` large transfers in the last hour.
/// Returns true if the account was flagged.
pub async fn flag_rmt_suspect(pool: &MySqlPool, cfg: &Config, account_id: i64) -> Result<bool> {
    let now = chrono::Utc::now().naive_utc();
    let threshold = cfg.economy.rmt_large_transfer_threshold;
    let new_days = cfg.economy.rmt_new_account_days;
    let burst = cfg.economy.rmt_burst_transfers;

    let created: Option<NaiveDateTime> =
        sqlx::query("SELECT created_at FROM account_gold WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(pool)
            .await?
            .map(|r| r.try_get("created_at").ok())
            .flatten();

    let new_account = created
        .map(|c| (now - c).num_days() < new_days)
        .unwrap_or(false);

    let large_in: i64 = sqlx::query(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) AS s FROM account_gold_log \
         WHERE account_id = ? AND type = 'transfer_in' AND amount > ?",
    )
    .bind(account_id)
    .bind(threshold)
    .fetch_one(pool)
    .await?
    .try_get("s")?;

    let one_hour_ago = now - chrono::Duration::hours(1);
    let recent_large: i64 = sqlx::query(
        "SELECT COUNT(*) AS c FROM account_gold_log \
         WHERE account_id = ? AND amount > ? AND type LIKE 'transfer%' AND created_at >= ?",
    )
    .bind(account_id)
    .bind(threshold)
    .bind(one_hour_ago)
    .fetch_one(pool)
    .await?
    .try_get("c")?;

    let reason = if new_account && large_in > threshold {
        format!("large transfer {large_in}g to account younger than {new_days} days")
    } else if recent_large > burst {
        format!("{recent_large} large transfers in the last hour")
    } else {
        return Ok(false);
    };

    sqlx::query("INSERT INTO rmt_suspects (account_id, reason) VALUES (?, ?)")
        .bind(account_id)
        .bind(&reason)
        .execute(pool)
        .await?;
    warn!(account_id, %reason, "RMT suspect flagged");
    Ok(true)
}

/// Reset the 24h spending tracker for every account (called after the tax run).
/// Returns the number of rows reset.
pub async fn reset_gold_activity(pool: &MySqlPool) -> Result<u64> {
    let now = chrono::Utc::now().naive_utc();
    let res = sqlx::query("UPDATE gold_activity SET spent_24h = 0, last_reset = ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Placeholder redistribution — logs the gold currently available in the pool.
pub async fn redistribute_tax_pool(pool: &MySqlPool) -> Result<i64> {
    let row = sqlx::query("SELECT pool FROM world_bank WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let pool_balance: i64 = row.try_get("pool")?;
    info!(pool_balance, "redistribute_tax_pool: gold available for redistribution");
    Ok(pool_balance)
}

/// Current economy health bucket based on the circulating/total ratio.
pub async fn get_economy_health(pool: &MySqlPool) -> Result<EconomyHealth> {
    let row = sqlx::query("SELECT total_gold, circulating FROM world_bank WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let total: i64 = row.try_get("total_gold")?;
    let circulating: i64 = row.try_get("circulating")?;
    if total <= 0 {
        return Ok(EconomyHealth::Healthy);
    }
    let ratio = circulating as f64 / total as f64;
    Ok(health_from_ratio(ratio))
}

/// Verify the closed-loop invariant and stamp `last_integrity_check`. Errors
/// if `circulating + pool != total` (possible dupe/exploit).
pub async fn hourly_integrity_check(pool: &MySqlPool, _cfg: &Config) -> Result<()> {
    let now = chrono::Utc::now().naive_utc();
    let row = sqlx::query("SELECT total_gold, circulating, pool FROM world_bank WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let total: i64 = row.try_get("total_gold")?;
    let circulating: i64 = row.try_get("circulating")?;
    let pool_bal: i64 = row.try_get("pool")?;

    sqlx::query("UPDATE world_bank SET last_integrity_check = ? WHERE id = 1")
        .bind(now)
        .execute(pool)
        .await?;

    if !check_invariant(circulating, pool_bal, total) {
        error!(circulating, pool = pool_bal, total, "INTEGRITY CHECK FAILED — possible dupe/exploit");
        return Err(Error::InvariantBroken {
            circulating,
            pool: pool_bal,
            cap: total,
        });
    }
    info!("hourly_integrity_check: invariant OK");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_mint_respects_pool() {
        assert!(can_mint_with(100, 1_000_000_000));
        assert!(!can_mint_with(0, 1_000_000_000)); // nothing to mint
        assert!(!can_mint_with(1, 0)); // empty pool
        assert!(can_mint_with(1_000_000_000, 1_000_000_000)); // exact cap
        assert!(!can_mint_with(1_000_000_001, 1_000_000_000)); // over cap
    }

    #[test]
    fn compute_tax_exempt_below_5000() {
        let tiers = default_tax_tiers();
        assert_eq!(compute_tax(0, &tiers), 0);
        assert_eq!(compute_tax(5_000, &tiers), 0);
    }

    #[test]
    fn compute_tax_low_bracket_only() {
        let tiers = default_tax_tiers();
        // 50_000 - 5_000 = 45_000 in the 0.5% bracket -> 225
        assert_eq!(compute_tax(50_000, &tiers), 225);
    }

    #[test]
    fn compute_tax_multiple_brackets() {
        let tiers = default_tax_tiers();
        // 60_000: (50_000-5_000)*0.005 + (60_000-50_000)*0.01 = 225 + 100 = 325
        assert_eq!(compute_tax(60_000, &tiers), 325);
        // 600_000: 225 + (500_000-50_000)*0.01 + (600_000-500_000)*0.02
        //        = 225 + 4_500 + 2_000 = 6_725
        assert_eq!(compute_tax(600_000, &tiers), 6_725);
    }

    #[test]
    fn health_thresholds() {
        assert_eq!(health_from_ratio(0.49), EconomyHealth::Healthy);
        assert_eq!(health_from_ratio(0.50), EconomyHealth::Monitor);
        assert_eq!(health_from_ratio(0.74), EconomyHealth::Monitor);
        assert_eq!(health_from_ratio(0.75), EconomyHealth::Warning);
        assert_eq!(health_from_ratio(0.89), EconomyHealth::Warning);
        assert_eq!(health_from_ratio(0.90), EconomyHealth::Critical);
        assert_eq!(health_from_ratio(0.99), EconomyHealth::Critical);
    }

    #[test]
    fn invariant_holds_and_breaks() {
        assert!(check_invariant(300_000_000, 700_000_000, 1_000_000_000));
        assert!(!check_invariant(300_000_001, 700_000_000, 1_000_000_000));
    }
}