//! Configuration loaded from `config.toml`.

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub api: ApiConfig,
    pub database: DatabaseConfig,
    pub economy: EconomyConfig,
    pub labor: LaborConfig,
    pub boss: BossConfig,
    pub honor: HonorConfig,
    pub gold_scaling: GoldScalingConfig,
    pub mounts: MountConfig,
    pub items: ItemConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub listen: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub connection: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EconomyConfig {
    pub total_gold_cap: i64,
    pub tax_run_schedule: String,
    pub rmt_large_transfer_threshold: i64,
    pub rmt_new_account_days: i64,
    pub rmt_burst_transfers: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LaborConfig {
    pub regen_interval_seconds: i64,
    pub regen_per_tick: i64,
    pub max_pool: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BossConfig {
    pub respawn_duration_seconds: i64,
    pub gold_base_per_member: i64,
    pub thunderstruck_chance_min: f64,
    pub thunderstruck_chance_max: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HonorConfig {
    pub multiplier: i64,
    pub shop_price_divisor: i64,
    pub skill_tome_cost: i64,
    pub skill_tome_skill_points: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoldScalingConfig {
    pub fish_base: i64,
    pub tradepack_gold_base: i64,
    pub tradepack_gilda: i64,
    pub coinpurse_flat_multiplier: i64,
    pub labor_spent_for_max: i64,
    pub max_multiplier: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MountConfig {
    pub default_speed: f32,
    pub dragon_speed: f32,
    pub cart_speed: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemConfig {
    pub dragon_mount_item_id: i64,
    pub blue_hauler_item_id: i64,
}

impl Config {
    /// Load configuration from a TOML file at `path`.
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("failed to read {path}: {e}")))?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tracked `config.example.toml` must parse cleanly into the current
    /// `Config` struct, so a freshly cloned repo boots without a local
    /// `config.toml`. Spot-checking one field per section guards against
    /// renamed/removed fields silently deserializing to defaults.
    #[test]
    fn example_config_parses_with_expected_values() {
        let raw = include_str!("../config.example.toml");
        let cfg: Config = toml::from_str(raw).expect("config.example.toml must parse");

        assert!(!cfg.api.listen.is_empty());
        assert!(!cfg.database.connection.is_empty());
        assert!(cfg.economy.total_gold_cap > 0);
        assert!(cfg.labor.max_pool > 0);
        assert!(cfg.boss.respawn_duration_seconds > 0);
        assert!(cfg.honor.multiplier > 0);
        assert!(cfg.gold_scaling.max_multiplier >= 1.0);
        assert!(cfg.mounts.default_speed > 0.0);
        // item ids are placeholders (0) in the example until confirmed from
        // client data, so only assert they're non-negative.
        assert!(cfg.items.dragon_mount_item_id >= 0);
        assert!(cfg.items.blue_hauler_item_id >= 0);
    }
}