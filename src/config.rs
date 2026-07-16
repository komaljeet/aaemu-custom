//! Configuration loaded from `config.toml`.

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
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