//! combat_normalization — universal Attack Power / Defense Power formulas.
//!
//! - `damage        = base_skill_damage * (1 + AP / 1000)`
//! - `damage_taken  = incoming_damage  * (1 - DP / (DP + 5000))`
//!
//! Armor type is cosmetic; no elemental resistances; no type counters.

use sqlx::{MySqlPool, Row};

use crate::error::Result;

// ---------------------------------------------------------------------------
// Pure helpers (no DB) — unit tested
// ---------------------------------------------------------------------------

/// Outgoing damage scaled by Attack Power.
pub fn compute_damage(attack_power: i64, base_skill_damage: i64) -> i64 {
    let raw = (base_skill_damage as f64) * (1.0 + (attack_power as f64) / 1000.0);
    raw.round() as i64
}

/// Incoming damage after Defense Power reduction.
pub fn compute_damage_taken(defense_power: i64, incoming_damage: i64) -> i64 {
    let dp = defense_power as f64;
    let reduced = (incoming_damage as f64) * (1.0 - dp / (dp + 5000.0));
    reduced.round() as i64
}

// ---------------------------------------------------------------------------
// DB-bound public API
// ---------------------------------------------------------------------------

/// Damage dealt by `attacker_id` for a skill with `base_skill_damage`.
///
/// Returns `None` when `attacker_id` has no `character_combat_stats` row yet
/// (unseeded). The HTTP layer serializes `None` as `null` so the C# caller can
/// fall back to its native calculation rather than silently treating an
/// unseeded attacker as "0 Attack Power" (which would strip their scaling).
pub async fn calculate_damage(
    pool: &MySqlPool,
    attacker_id: i64,
    base_skill_damage: i64,
) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT attack_power FROM character_combat_stats WHERE character_id = ?")
        .bind(attacker_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let ap: i64 = row.try_get("attack_power")?;
    Ok(Some(compute_damage(ap, base_skill_damage)))
}

/// Damage taken by `defender_id` from `incoming_damage`.
///
/// Returns `None` when `defender_id` has no `character_combat_stats` row yet
/// (unseeded). The HTTP layer serializes `None` as `null` so the C# caller
/// falls back to native armor mitigation — without this, an unseeded defender
/// (DP = 0) would take full damage, regressing them versus native behavior.
pub async fn calculate_damage_taken(
    pool: &MySqlPool,
    defender_id: i64,
    incoming_damage: i64,
) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT defense_power FROM character_combat_stats WHERE character_id = ?")
        .bind(defender_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let dp: i64 = row.try_get("defense_power")?;
    Ok(Some(compute_damage_taken(dp, incoming_damage)))
}

/// `(attack_power, defense_power)` for a character. Defaults to `(0, 0)` if no
/// row exists yet.
pub async fn get_combat_stats(pool: &MySqlPool, character_id: i64) -> Result<(i64, i64)> {
    let row = sqlx::query(
        "SELECT attack_power, defense_power FROM character_combat_stats WHERE character_id = ?",
    )
    .bind(character_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok((0, 0));
    };
    let ap: i64 = row.try_get("attack_power")?;
    let dp: i64 = row.try_get("defense_power")?;
    Ok((ap, dp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_scales_with_ap() {
        assert_eq!(compute_damage(0, 1000), 1000); // x1
        assert_eq!(compute_damage(1000, 1000), 2000); // x2
        assert_eq!(compute_damage(500, 1000), 1500); // x1.5
        assert_eq!(compute_damage(250, 1000), 1250);
    }

    #[test]
    fn damage_taken_reduces_with_dp() {
        // DP=0 -> full damage
        assert_eq!(compute_damage_taken(0, 1000), 1000);
        // DP=5000 -> 1 - 5000/10000 = 0.5 -> 500
        assert_eq!(compute_damage_taken(5000, 1000), 500);
        // DP=15000 -> 1 - 15000/20000 = 0.25 -> 250
        assert_eq!(compute_damage_taken(15000, 1000), 250);
        // Very high DP approaches but never reaches full mitigation.
        assert!(compute_damage_taken(1_000_000, 1000) <= 5);
    }

    #[test]
    fn damage_zero_base_is_zero() {
        // base skill damage 0 yields 0 regardless of Attack Power.
        assert_eq!(compute_damage(1_000_000, 0), 0);
        assert_eq!(compute_damage(0, 0), 0);
    }

    #[test]
    fn damage_taken_zero_incoming_is_zero() {
        assert_eq!(compute_damage_taken(5_000, 0), 0);
        assert_eq!(compute_damage_taken(0, 0), 0);
    }
}