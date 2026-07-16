//! api — HTTP surface the C# AAEmu server calls to drive the custom systems.
//!
//! Listens on `config.api.listen` (default 127.0.0.1:1281). All endpoints are
//! JSON. Errors map to HTTP status codes:
//!   404 NotFound, 400 insufficient labor/honor/pool, 409 invariant/already-init,
//!   500 everything else.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::MySqlPool;

use crate::config::Config;
use crate::error::Error;
use crate::{boss_respawn, combat_normalization, gold_scaling, honor, labor, starter_perks,
            vehicle_mount_system, world_bank};

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub cfg: Arc<Config>,
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn map_err(e: Error) -> (StatusCode, String) {
    let code = match &e {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::InsufficientLabor { .. }
        | Error::InsufficientHonor { .. }
        | Error::InsufficientPool { .. } => StatusCode::BAD_REQUEST,
        Error::AlreadyInitialized | Error::InvariantBroken { .. } => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

// --- request bodies --------------------------------------------------------

#[derive(Deserialize)]
struct MintBody {
    account_id: i64,
    character_id: Option<i64>,
    amount: i64,
}

#[derive(Deserialize)]
struct LogTxBody {
    account_id: i64,
    character_id: Option<i64>,
    tx_type: String,
    amount: i64,
}

#[derive(Deserialize)]
struct SpendBody {
    account_id: i64,
    amount: i64,
}

#[derive(Deserialize)]
struct AccountCharacterBody {
    account_id: i64,
    character_id: i64,
}

#[derive(Deserialize)]
struct CoinpurseBody {
    account_id: i64,
    base_gold: i64,
}

#[derive(Deserialize)]
struct RaidMemberBody {
    character_id: i64,
    account_id: i64,
}

#[derive(Deserialize)]
struct BossKillBody {
    boss_id: i64,
    raid_id: i64,
    #[serde(default)]
    members: Vec<RaidMemberBody>,
}

#[derive(Deserialize)]
struct HonorEventBody {
    account_id: i64,
    base_honor: i64,
}

#[derive(Deserialize)]
struct CombatDamageBody {
    attacker_id: i64,
    base_skill_damage: i64,
}

#[derive(Deserialize)]
struct DamageTakenBody {
    defender_id: i64,
    incoming_damage: i64,
}

#[derive(Deserialize)]
struct BuffsQuery {
    buffs: Option<f32>,
}

// --- handlers --------------------------------------------------------------

async fn health() -> &'static str {
    "ok"
}

async fn init_db(State(st): State<AppState>) -> ApiResult {
    let sql = std::fs::read_to_string("schema.sql")
        .map_err(|e| Error::Io(e))
        .map_err(map_err)?;
    for stmt in sql.split(';') {
        let body: String = stmt
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = body.trim();
        if trimmed.is_empty() {
            continue;
        }
        sqlx::query(trimmed).execute(&st.pool).await.map_err(Error::Db).map_err(map_err)?;
    }
    world_bank::initialize_world_bank(&st.pool, &st.cfg).await.map_err(map_err)?;
    let v = vehicle_mount_system::initialize_vehicle_defaults(&st.pool, &st.cfg)
        .await
        .map_err(map_err)?;
    let m = vehicle_mount_system::initialize_mount_defaults(&st.pool, &st.cfg)
        .await
        .map_err(map_err)?;
    Ok(json!({"schema": "applied", "vehicles": v, "mounts": m}).into())
}

async fn wb_integrity(State(st): State<AppState>) -> ApiResult {
    world_bank::hourly_integrity_check(&st.pool, &st.cfg).await.map_err(map_err)?;
    Ok(json!({"invariant": "ok"}).into())
}

async fn wb_health(State(st): State<AppState>) -> ApiResult {
    let h = world_bank::get_economy_health(&st.pool).await.map_err(map_err)?;
    Ok(json!({"health": h.as_str()}).into())
}

async fn wb_tax_run(State(st): State<AppState>) -> ApiResult {
    let total = world_bank::run_daily_tax(&st.pool, &st.cfg).await.map_err(map_err)?;
    Ok(json!({"collected": total}).into())
}

async fn wb_mint(State(st): State<AppState>, Json(b): Json<MintBody>) -> ApiResult {
    world_bank::log_transaction(&st.pool, b.account_id, b.character_id, "reward", b.amount)
        .await
        .map_err(map_err)?;
    Ok(json!({"minted": b.amount}).into())
}

async fn wb_log(State(st): State<AppState>, Json(b): Json<LogTxBody>) -> ApiResult {
    world_bank::log_transaction(&st.pool, b.account_id, b.character_id, &b.tx_type, b.amount)
        .await
        .map_err(map_err)?;
    Ok(json!({"logged": true}).into())
}

async fn wb_rmt(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let flagged = world_bank::flag_rmt_suspect(&st.pool, &st.cfg, account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": account_id, "flagged": flagged}).into())
}

async fn labor_tick(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let pool = labor::tick_labor_regen(&st.pool, &st.cfg, account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": account_id, "pool": pool}).into())
}

async fn labor_spend(State(st): State<AppState>, Json(b): Json<SpendBody>) -> ApiResult {
    let pool = labor::spend_labor(&st.pool, b.account_id, b.amount)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": b.account_id, "pool": pool}).into())
}

/// Notify-only labor spend: advances `total_labor_spent` (and thus the gold
/// multiplier) without the sidecar's own pool having to cover it. Used by the
/// C# server's `AccountManager.UpdateLabor` hook.
async fn labor_spent(State(st): State<AppState>, Json(b): Json<SpendBody>) -> ApiResult {
    let total = labor::record_labor_spent(&st.pool, b.account_id, b.amount)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": b.account_id, "total_spent": total}).into())
}

async fn labor_get(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let pool = labor::get_labor(&st.pool, account_id).await.map_err(map_err)?;
    Ok(json!({"account_id": account_id, "pool": pool}).into())
}

async fn gold_multiplier(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let m = gold_scaling::get_multiplier(&st.pool, &st.cfg, account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": account_id, "multiplier": m}).into())
}

async fn gold_fish(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let gold = gold_scaling::calculate_fish_gold(&st.pool, &st.cfg, account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": account_id, "gold": gold}).into())
}

async fn gold_tradepack(State(st): State<AppState>, Path(account_id): Path<i64>) -> ApiResult {
    let (gold, gilda) = gold_scaling::calculate_tradepack_reward(&st.pool, &st.cfg, account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": account_id, "gold": gold, "gilda": gilda}).into())
}

async fn gold_coinpurse(State(st): State<AppState>, Json(b): Json<CoinpurseBody>) -> ApiResult {
    let gold = gold_scaling::calculate_coinpurse_gold(&st.pool, &st.cfg, b.account_id, b.base_gold)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": b.account_id, "gold": gold}).into())
}

async fn perks_grant(State(st): State<AppState>, Json(b): Json<AccountCharacterBody>) -> ApiResult {
    let granted = starter_perks::grant_starter_perks(&st.pool, &st.cfg, b.character_id, b.account_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"character_id": b.character_id, "granted": granted}).into())
}

async fn perks_flight(State(st): State<AppState>, Path(mount_id): Path<i64>) -> ApiResult {
    let updated = starter_perks::grant_flight_capability(&st.pool, mount_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"mount_id": mount_id, "updated": updated}).into())
}

async fn boss_kill(State(st): State<AppState>, Json(b): Json<BossKillBody>) -> ApiResult {
    let members: Vec<(i64, i64)> = b
        .members
        .iter()
        .map(|m| (m.character_id, m.account_id))
        .collect();
    let loot = boss_respawn::on_boss_killed(&st.pool, &st.cfg, b.boss_id, b.raid_id, &members)
        .await
        .map_err(map_err)?;
    Ok(json!({"boss_id": b.boss_id, "raid_id": b.raid_id, "loot": loot}).into())
}

async fn boss_ready(State(st): State<AppState>) -> ApiResult {
    let ids = boss_respawn::get_bosses_ready_to_spawn(&st.pool)
        .await
        .map_err(map_err)?;
    Ok(json!({"bosses": ids}).into())
}

async fn honor_event(State(st): State<AppState>, Json(b): Json<HonorEventBody>) -> ApiResult {
    let h = honor::grant_event_honor(&st.pool, &st.cfg, b.account_id, b.base_honor)
        .await
        .map_err(map_err)?;
    Ok(json!({"account_id": b.account_id, "honor": h}).into())
}

async fn honor_tome(State(st): State<AppState>, Json(b): Json<AccountCharacterBody>) -> ApiResult {
    let pts = honor::use_skill_point_tome(&st.pool, &st.cfg, b.account_id, b.character_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"character_id": b.character_id, "skill_points": pts}).into())
}

async fn honor_price(State(st): State<AppState>, Path(item_id): Path<i64>) -> ApiResult {
    let price = honor::get_shop_price(&st.pool, &st.cfg, item_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"item_id": item_id, "price": price}).into())
}

async fn combat_damage(State(st): State<AppState>, Json(b): Json<CombatDamageBody>) -> ApiResult {
    let dmg = combat_normalization::calculate_damage(&st.pool, b.attacker_id, b.base_skill_damage)
        .await
        .map_err(map_err)?;
    Ok(json!({"attacker_id": b.attacker_id, "damage": dmg}).into())
}

async fn combat_damage_taken(State(st): State<AppState>, Json(b): Json<DamageTakenBody>) -> ApiResult {
    let taken =
        combat_normalization::calculate_damage_taken(&st.pool, b.defender_id, b.incoming_damage)
            .await
            .map_err(map_err)?;
    Ok(json!({"defender_id": b.defender_id, "damage_taken": taken}).into())
}

async fn combat_stats(State(st): State<AppState>, Path(character_id): Path<i64>) -> ApiResult {
    let (ap, dp) = combat_normalization::get_combat_stats(&st.pool, character_id)
        .await
        .map_err(map_err)?;
    Ok(json!({"character_id": character_id, "attack_power": ap, "defense_power": dp}).into())
}

async fn vehicle_speed(
    State(st): State<AppState>,
    Path(vehicle_id): Path<i64>,
    Query(q): Query<BuffsQuery>,
) -> ApiResult {
    let s = vehicle_mount_system::get_vehicle_speed(&st.pool, &st.cfg, vehicle_id, q.buffs.unwrap_or(0.0))
        .await
        .map_err(map_err)?;
    Ok(json!({"vehicle_id": vehicle_id, "speed": s}).into())
}

async fn mount_speed(
    State(st): State<AppState>,
    Path(mount_id): Path<i64>,
    Query(q): Query<BuffsQuery>,
) -> ApiResult {
    let s = vehicle_mount_system::get_mount_speed(&st.pool, &st.cfg, mount_id, q.buffs.unwrap_or(0.0))
        .await
        .map_err(map_err)?;
    Ok(json!({"mount_id": mount_id, "speed": s}).into())
}

// --- router ----------------------------------------------------------------

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/init-db", post(init_db))
        // world_bank
        .route("/world-bank/integrity", post(wb_integrity))
        .route("/world-bank/health", get(wb_health))
        .route("/world-bank/tax/run", post(wb_tax_run))
        .route("/world-bank/mint", post(wb_mint))
        .route("/world-bank/log", post(wb_log))
        .route("/world-bank/rmt/:account_id", post(wb_rmt))
        // labor
        .route("/labor/tick/:account_id", post(labor_tick))
        .route("/labor/spend", post(labor_spend))
        .route("/labor/spent", post(labor_spent))
        .route("/labor/:account_id", get(labor_get))
        // gold_scaling
        .route("/gold/multiplier/:account_id", get(gold_multiplier))
        .route("/gold/fish/:account_id", get(gold_fish))
        .route("/gold/tradepack/:account_id", get(gold_tradepack))
        .route("/gold/coinpurse", post(gold_coinpurse))
        // starter_perks
        .route("/perks/grant", post(perks_grant))
        .route("/perks/flight/:mount_id", post(perks_flight))
        // boss_respawn
        .route("/boss/kill", post(boss_kill))
        .route("/boss/ready", get(boss_ready))
        // honor
        .route("/honor/event", post(honor_event))
        .route("/honor/tome", post(honor_tome))
        .route("/honor/price/:item_id", get(honor_price))
        // combat_normalization
        .route("/combat/damage", post(combat_damage))
        .route("/combat/damage-taken", post(combat_damage_taken))
        .route("/combat/stats/:character_id", get(combat_stats))
        // vehicle / mount
        .route("/vehicle/speed/:vehicle_id", get(vehicle_speed))
        .route("/mount/speed/:mount_id", get(mount_speed))
        .with_state(state)
}

/// Bind and serve the API. Runs until the listener is dropped.
pub async fn serve(state: AppState, addr: &str) -> Result<(), Error> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Other(format!("failed to bind {addr}: {e}")))?;
    tracing::info!(%addr, "API listening");
    axum::serve(listener, router(state))
        .await
        .map_err(|e| Error::Other(format!("api serve error: {e}")))?;
    Ok(())
}