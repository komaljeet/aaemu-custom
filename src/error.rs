//! Shared error type for all aaemu-custom modules.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml deserialize error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("world bank invariant broken: circulating={circulating} pool={pool} cap={cap}")]
    InvariantBroken {
        circulating: i64,
        pool: i64,
        cap: i64,
    },

    #[error("insufficient gold in pool to mint {requested}, pool has {available}")]
    InsufficientPool { requested: i64, available: i64 },

    #[error("not enough labor: have {have}, need {need}")]
    InsufficientLabor { have: i64, need: i64 },

    #[error("not enough honor: have {have}, need {need}")]
    InsufficientHonor { have: i64, need: i64 },

    #[error("world bank already initialized")]
    AlreadyInitialized,

    #[error("row not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;