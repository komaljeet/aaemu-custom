//! aaemu-custom — custom game server systems for an ArcheAge 1.2 private server.
//!
//! This crate is a sidecar to the C# AAEmu server. It owns custom gameplay
//! systems (closed-loop economy, labor, gold scaling, starter perks, boss
//! respawn, honor, combat normalization, vehicles/mounts) and reads/writes the
//! same `aaemu_game` MySQL database. Each module exposes a clean public API of
//! async functions taking a `sqlx::MySqlPool` plus the relevant `Config`.
//!
//! Pure calculation logic is split into `*_raw` helpers so it can be unit
//! tested without a live database.

pub mod api;
pub mod boss_respawn;
pub mod combat_normalization;
pub mod config;
pub mod error;
pub mod gold_scaling;
pub mod honor;
pub mod labor;
pub mod starter_perks;
pub mod vehicle_mount_system;
pub mod world_bank;

pub use config::Config;
pub use error::{Error, Result};