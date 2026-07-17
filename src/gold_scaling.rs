//! gold_scaling — labor-scaled gold rewards.
//!
//! Gold multiplier scales linearly with total labor spent, from x1 at 0 labor
//! to x4 at 230_000 labor. All rewards check the world bank can mint before
//! paying out; rewards pause (return 0) when the pool is empty.

use sqlx::{MySqlPool, Row};

use crate::config::Config;
use crate::error::Result;
use crate::world_bank;

// ---------------------------------------------------------------------------
// Pure helpers (no DB) — unit tested
// ---------------------------------------------------------------------------

/// Multiplier from total labor spent, clamped to `[1, max_multiplier]`.
pub fn multiplier_from_labor(labor_spent: i64, cfg: &Config) -> f64 {
    let denom = cfg.gold_scaling.labor_spent_for_max as f64;
    let raw = 1.0 + (labor_spent as f64 / denom) * 3.0;
    raw.clamp(1.0, cfg.gold_scaling.max_multiplier)
}

/// Fish gold = base * multiplier.
pub fn fish_gold(multiplier: f64, base: i64) -> i64 {
    ((base as f64) * multiplier).round() as i64
}

/// Tradepack gold = base * multiplier (gilda is flat and never scales).
pub fn tradepack_gold(multiplier: f64, base: i64) -> i64 {
    ((base as f64) * multiplier).round() as i64
}

/// Coinpurse gold = base * flat_multiplier * labor_multiplier.
pub fn coinpurse_gold(multiplier: f64, base: i64, flat_multiplier: i64) -> i64 {
    ((base as f64) * (flat_multiplier as f64) * multiplier).round() as i64
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// Current gold multiplier for an account based on total labor spent.
pub async fn get_multiplier(pool: &MySqlPool, cfg: &Config, account_id: i64) -> Result<f64> {
    let row =
        sqlx::query("SELECT total_labor_spent FROM account_labor WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
    let spent: i64 = row
        .map(|r| r.try_get::<i64, _>("total_labor_spent").unwrap_or(0))
        .unwrap_or(0);
    Ok(multiplier_from_labor(spent, cfg))
}

/// Fish reward. Returns 0 if the world bank can't mint the gold.
pub async fn calculate_fish_gold(
    pool: &MySqlPool,
    cfg: &Config,
    account_id: i64,
) -> Result<i64> {
    let m = get_multiplier(pool, cfg, account_id).await?;
    let amount = fish_gold(m, cfg.gold_scaling.fish_base);
    if !world_bank::can_mint_gold(pool, amount).await? {
        return Ok(0);
    }
    Ok(amount)
}

/// Tradepack reward `(gold, gilda)`. Gilda is flat and never scales; gold is 0
/// if the world bank can't mint it.
pub async fn calculate_tradepack_reward(
    pool: &MySqlPool,
    cfg: &Config,
    account_id: i64,
) -> Result<(i64, i64)> {
    let m = get_multiplier(pool, cfg, account_id).await?;
    let gold = tradepack_gold(m, cfg.gold_scaling.tradepack_gold_base);
    let gilda = cfg.gold_scaling.tradepack_gilda;
    if !world_bank::can_mint_gold(pool, gold).await? {
        return Ok((0, gilda));
    }
    Ok((gold, gilda))
}

/// Coinpurse reward (x20 flat, then labor multiplier). Returns 0 if the world
/// bank can't mint the gold.
pub async fn calculate_coinpurse_gold(
    pool: &MySqlPool,
    cfg: &Config,
    account_id: i64,
    base_gold: i64,
) -> Result<i64> {
    let m = get_multiplier(pool, cfg, account_id).await?;
    let amount = coinpurse_gold(m, base_gold, cfg.gold_scaling.coinpurse_flat_multiplier);
    if !world_bank::can_mint_gold(pool, amount).await? {
        return Ok(0);
    }
    Ok(amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg() -> Config {
        // Minimal config with only the fields this module reads. Tests that
        // touch other modules would build a fuller Config.
        let raw = std::fs::read_to_string("config.toml").unwrap_or_else(|_| {
            // Fallback if config.toml isn't present (it's gitignored) or cwd
            // isn't the crate root during testing. config.example.toml is the
            // tracked template with every field, so it parses identically.
            include_str!("../config.example.toml").to_string()
        });
        toml::from_str(&raw).unwrap()
    }

    #[test]
    fn multiplier_scales_linearly() {
        let c = cfg();
        let eps = 1e-9;
        assert!((multiplier_from_labor(0, &c) - 1.0).abs() < eps);
        // 115_000 is half of 230_000 -> 1 + 0.5*3 = 2.5
        assert!((multiplier_from_labor(115_000, &c) - 2.5).abs() < eps);
        assert!((multiplier_from_labor(230_000, &c) - 4.0).abs() < eps);
    }

    #[test]
    fn multiplier_clamps_at_max() {
        let c = cfg();
        let eps = 1e-9;
        assert!((multiplier_from_labor(230_001, &c) - 4.0).abs() < eps);
        assert!((multiplier_from_labor(1_000_000, &c) - 4.0).abs() < eps);
    }

    #[test]
    fn fish_gold_uses_multiplier() {
        assert_eq!(fish_gold(1.0, 50), 50);
        assert_eq!(fish_gold(4.0, 50), 200);
        assert_eq!(fish_gold(2.5, 50), 125);
    }

    #[test]
    fn coinpurse_applies_flat_then_multiplier() {
        // base 10, x20 flat, x4 multiplier -> 800
        assert_eq!(coinpurse_gold(4.0, 10, 20), 800);
        // base 10, x20 flat, x1 -> 200
        assert_eq!(coinpurse_gold(1.0, 10, 20), 200);
    }

    #[test]
    fn tradepack_gold_scales_gilda_does_not() {
        assert_eq!(tradepack_gold(4.0, 100), 400);
        assert_eq!(tradepack_gold(1.0, 100), 100);
    }

    #[test]
    fn multiplier_negative_labor_clamps_to_one() {
        let c = cfg();
        let eps = 1e-9;
        // Negative labor (shouldn't happen, but a bad row could) must not pull
        // the multiplier below 1.0.
        assert!((multiplier_from_labor(-1, &c) - 1.0).abs() < eps);
        assert!((multiplier_from_labor(-100_000, &c) - 1.0).abs() < eps);
    }

    #[test]
    fn multiplier_at_exact_inflection_hits_cap() {
        let c = cfg();
        let eps = 1e-9;
        // labor_spent_for_max is exactly the configured inflection point.
        let at = c.gold_scaling.labor_spent_for_max;
        assert!(
            (multiplier_from_labor(at, &c) - c.gold_scaling.max_multiplier).abs() < eps
        );
    }

    #[test]
    fn fish_gold_rounds_half_away_from_zero() {
        // 10 * 2.25 = 22.5 -> rounds to 23 (2.25 is exactly representable).
        assert_eq!(fish_gold(2.25, 10), 23);
        assert_eq!(fish_gold(1.0, 0), 0); // zero base
    }

    #[test]
    fn coinpurse_and_tradepack_zero_base() {
        assert_eq!(coinpurse_gold(4.0, 0, 20), 0);
        assert_eq!(tradepack_gold(4.0, 0), 0);
    }
}