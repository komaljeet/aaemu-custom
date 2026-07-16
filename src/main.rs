//! aaemu-custom binary — DB bootstrap and the scheduler loop.
//!
//! Usage:
//!   aaemu-custom                 run the scheduler (integrity / boss / labor / tax)
//!   aaemu-custom --init-db       apply schema.sql, init the world bank, apply
//!                                vehicle/mount defaults, then exit
//!   aaemu-custom --config X.toml use a different config file
//!   aaemu-custom --schema Y.sql  schema file for --init-db (default schema.sql)

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{Datelike, Local, Timelike};
use sqlx::{MySqlPool, Row};
use tracing::{error, info, warn};

use aaemu_custom::{boss_respawn, config::Config, error::Result, labor, vehicle_mount_system, world_bank};

#[derive(Debug, Default)]
struct Args {
    config: String,
    schema: String,
    init_db: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        config: "config.toml".into(),
        schema: "schema.sql".into(),
        init_db: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--init-db" => a.init_db = true,
            "--config" => {
                if let Some(v) = it.next() {
                    a.config = v;
                }
            }
            "--schema" => {
                if let Some(v) = it.next() {
                    a.schema = v;
                }
            }
            other => warn!(arg = other, "ignoring unknown argument"),
        }
    }
    a
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aaemu_custom=info,sqlx=warn".into()),
        )
        .init();

    let args = parse_args();
    let cfg = Config::load(&args.config)?;
    info!(connection = %cfg.database.connection, "connecting to MySQL");
    let pool = MySqlPool::connect(&cfg.database.connection).await?;
    info!("connected");

    if args.init_db {
        return init_db(&pool, &cfg, &args.schema).await;
    }

    run_scheduler(&pool, &cfg).await
}

/// Apply the schema, bootstrap the world bank, and seed vehicle/mount defaults.
async fn init_db(pool: &MySqlPool, cfg: &Config, schema_path: &str) -> Result<()> {
    info!(schema_path, "applying schema");
    let sql = std::fs::read_to_string(schema_path)?;
    for stmt in sql.split(';') {
        // Strip `--` line comments so a chunk that is only comments is skipped
        // and a chunk that is comment + statement still executes.
        let body: String = stmt
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = body.trim();
        if trimmed.is_empty() {
            continue;
        }
        sqlx::query(trimmed).execute(pool).await?;
    }
    info!("schema applied");

    world_bank::initialize_world_bank(pool, cfg).await?;
    world_bank::hourly_integrity_check(pool, cfg).await?;
    let v = vehicle_mount_system::initialize_vehicle_defaults(pool, cfg).await?;
    let m = vehicle_mount_system::initialize_mount_defaults(pool, cfg).await?;
    info!(vehicles = v, mounts = m, "defaults applied — --init-db complete");
    Ok(())
}

/// Long-running scheduler: integrity / boss / labor / tax ticks until Ctrl+C.
async fn run_scheduler(pool: &MySqlPool, cfg: &Config) -> Result<()> {
    // Initial integrity stamp on startup.
    if let Err(e) = world_bank::hourly_integrity_check(pool, cfg).await {
        error!(error = %e, "startup integrity check failed");
    }

    let last_tax_day = AtomicI64::new(i64::MIN);

    // Hourly closed-loop integrity check.
    let p = pool.clone();
    let c = cfg.clone();
    let integrity = tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            t.tick().await;
            if let Err(e) = world_bank::hourly_integrity_check(&p, &c).await {
                error!(error = %e, "integrity check failed");
            }
        }
    });

    // Per-minute boss spawn tick.
    let p = pool.clone();
    let boss = tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            t.tick().await;
            match boss_respawn::tick_boss_spawns(&p).await {
                Ok(0) => {}
                Ok(n) => info!(ready = n, "boss spawns triggered"),
                Err(e) => error!(error = %e, "boss tick failed"),
            }
        }
    });

    // Labor regen tick (every regen interval, across all accounts).
    let p = pool.clone();
    let c = cfg.clone();
    let labor_tick = tokio::spawn(async move {
        let mut t =
            tokio::time::interval(std::time::Duration::from_secs(c.labor.regen_interval_seconds as u64));
        loop {
            t.tick().await;
            match sqlx::query("SELECT account_id FROM account_labor").fetch_all(&p).await {
                Ok(rows) => {
                    for r in rows {
                        let id: i64 = r.try_get("account_id").unwrap_or(0);
                        if let Err(e) = labor::tick_labor_regen(&p, &c, id).await {
                            error!(account_id = id, error = %e, "labor tick failed");
                        }
                    }
                }
                Err(e) => error!(error = %e, "labor account fetch failed"),
            }
        }
    });

    // Daily tax run — checked hourly, fires once per day at the scheduled hour.
    let p = pool.clone();
    let c = cfg.clone();
    let tax = tokio::spawn(async move {
        let scheduled_hour = parse_scheduled_hour(&c.economy.tax_run_schedule);
        let mut t = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            t.tick().await;
            let now = Local::now();
            let today = now.date_naive().num_days_from_ce() as i64;
            if now.hour() as i64 == scheduled_hour
                && last_tax_day.swap(today, Ordering::Relaxed) != today
            {
                info!("starting daily tax run");
                match world_bank::run_daily_tax(&p, &c).await {
                    Ok(collected) => info!(collected, "daily tax run finished"),
                    Err(e) => error!(error = %e, "daily tax run failed"),
                }
            }
        }
    });

    info!("scheduler running — press Ctrl+C to shut down");
    tokio::signal::ctrl_c().await.ok();
    info!("Ctrl+C received — shutting down");
    integrity.abort();
    boss.abort();
    labor_tick.abort();
    tax.abort();
    Ok(())
}

/// Best-effort parse of the cron hour field `SS MM HH ...` → HH. Falls back to 3.
fn parse_scheduled_hour(schedule: &str) -> i64 {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() >= 3 {
        if let Ok(h) = parts[2].parse::<i64>() {
            return h;
        }
    }
    warn!(schedule, "could not parse tax schedule hour, defaulting to 03:00");
    3
}