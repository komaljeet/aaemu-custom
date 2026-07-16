-- aaemu-custom schema
-- All tables live in the aaemu_game database alongside the C# AAEmu server's
-- tables. Every statement is idempotent (CREATE TABLE IF NOT EXISTS) so this
-- file can be re-run safely.
--
-- Apply with:  mysql -u root -p aaemu_game < schema.sql
--   or via the binary:  aaemu-custom --init-db

-- ============================================================================
-- world_bank : closed-loop economy aggregate
-- ============================================================================
CREATE TABLE IF NOT EXISTS world_bank (
    id                    TINYINT UNSIGNED NOT NULL PRIMARY KEY DEFAULT 1,
    total_gold            BIGINT          NOT NULL,
    circulating           BIGINT          NOT NULL,
    pool                  BIGINT          NOT NULL,
    taxed_today           BIGINT          NOT NULL DEFAULT 0,
    last_integrity_check  DATETIME(3)     NULL,
    updated_at            DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    CONSTRAINT chk_world_bank_singleton CHECK (id = 1)
) ENGINE=InnoDB;

-- ============================================================================
-- account_gold : per-account liquid gold (source of truth for player balances)
-- ============================================================================
CREATE TABLE IF NOT EXISTS account_gold (
    account_id   BIGINT      NOT NULL PRIMARY KEY,
    balance      BIGINT      NOT NULL DEFAULT 0,
    created_at   DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at   DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
) ENGINE=InnoDB;

-- ============================================================================
-- account_gold_log : full transaction audit
-- ============================================================================
CREATE TABLE IF NOT EXISTS account_gold_log (
    id            BIGINT        NOT NULL AUTO_INCREMENT PRIMARY KEY,
    account_id    BIGINT        NOT NULL,
    character_id  BIGINT        NULL,
    type          VARCHAR(32)   NOT NULL,
    amount        BIGINT        NOT NULL,
    created_at    DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    INDEX idx_account_gold_log_account (account_id),
    INDEX idx_account_gold_log_created (created_at)
) ENGINE=InnoDB;

-- ============================================================================
-- gold_activity : 24h spending tracker (gold-in-motion exemption)
-- ============================================================================
CREATE TABLE IF NOT EXISTS gold_activity (
    account_id   BIGINT      NOT NULL PRIMARY KEY,
    spent_24h    BIGINT      NOT NULL DEFAULT 0,
    last_reset   DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
) ENGINE=InnoDB;

-- ============================================================================
-- tax_tiers : configurable daily wealth tax brackets (rate_pct as percent)
-- ============================================================================
CREATE TABLE IF NOT EXISTS tax_tiers (
    id        INT          NOT NULL AUTO_INCREMENT PRIMARY KEY,
    min_gold  BIGINT       NOT NULL,
    max_gold  BIGINT       NULL,
    rate_pct  DECIMAL(6,4) NOT NULL
) ENGINE=InnoDB;

INSERT IGNORE INTO tax_tiers (id, min_gold, max_gold, rate_pct) VALUES
    (1, 0,      5000,    0.0000),
    (2, 5001,   50000,   0.5000),
    (3, 50001,  500000,  1.0000),
    (4, 500001, NULL,    2.0000);

-- ============================================================================
-- rmt_suspects : flagged accounts from flag_rmt_suspect
-- ============================================================================
CREATE TABLE IF NOT EXISTS rmt_suspects (
    id          BIGINT        NOT NULL AUTO_INCREMENT PRIMARY KEY,
    account_id  BIGINT        NOT NULL,
    reason      VARCHAR(255)  NOT NULL,
    flagged_at  DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    INDEX idx_rmt_suspects_account (account_id)
) ENGINE=InnoDB;

-- ============================================================================
-- account_labor : shared labor pool per account
-- ============================================================================
CREATE TABLE IF NOT EXISTS account_labor (
    account_id          BIGINT      NOT NULL PRIMARY KEY,
    pool                INT         NOT NULL DEFAULT 0,
    last_regen_tick     DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    total_labor_spent   BIGINT      NOT NULL DEFAULT 0
) ENGINE=InnoDB;

-- ============================================================================
-- mount_metadata : wing / flight metadata for starter perks
-- ============================================================================
CREATE TABLE IF NOT EXISTS mount_metadata (
    mount_id       BIGINT       NOT NULL PRIMARY KEY,
    has_wings      BOOLEAN      NOT NULL DEFAULT FALSE,
    flight_type    ENUM('NONE','GLIDE','TRUE_FLIGHT') NOT NULL DEFAULT 'NONE',
    is_permanent   BOOLEAN      NOT NULL DEFAULT FALSE
) ENGINE=InnoDB;

-- Dragon mount (true flight, permanent) + blue hauler (permanent, no wings).
INSERT IGNORE INTO mount_metadata (mount_id, has_wings, flight_type, is_permanent) VALUES
    (0, TRUE,  'TRUE_FLIGHT', TRUE),   -- dragon_mount_item_id (set in config.toml)
    (0, FALSE, 'NONE',        TRUE);   -- blue_hauler_item_id  (set in config.toml)

-- ============================================================================
-- granted_perks : starter perk entitlements (idempotent grant guard)
-- ============================================================================
CREATE TABLE IF NOT EXISTS granted_perks (
    id            BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    character_id  BIGINT       NOT NULL,
    account_id    BIGINT       NOT NULL,
    perk_type     VARCHAR(32)  NOT NULL,
    item_id       BIGINT       NOT NULL,
    is_permanent  BOOLEAN      NOT NULL DEFAULT TRUE,
    granted_at    DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    UNIQUE KEY uq_granted_perks_char_perk (character_id, perk_type, item_id)
) ENGINE=InnoDB;

-- ============================================================================
-- boss_spawn_state : world boss respawn timers
-- ============================================================================
CREATE TABLE IF NOT EXISTS boss_spawn_state (
    boss_id            BIGINT       NOT NULL PRIMARY KEY,
    last_killed_at     DATETIME(3)  NULL,
    next_spawn_at      DATETIME(3)  NULL,
    killed_by_raid_id  BIGINT       NULL
) ENGINE=InnoDB;

-- ============================================================================
-- raid_members : raid composition at kill time (pushed by the game server)
-- ============================================================================
CREATE TABLE IF NOT EXISTS raid_members (
    raid_id       BIGINT      NOT NULL,
    character_id  BIGINT      NOT NULL,
    account_id    BIGINT      NOT NULL,
    PRIMARY KEY (raid_id, character_id)
) ENGINE=InnoDB;

-- ============================================================================
-- boss_loot_log : personal loot distributed to killing raid members
-- ============================================================================
CREATE TABLE IF NOT EXISTS boss_loot_log (
    id             BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    boss_id        BIGINT       NOT NULL,
    raid_id        BIGINT       NOT NULL,
    character_id   BIGINT       NOT NULL,
    gold           BIGINT       NOT NULL DEFAULT 0,
    thunderstruck  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at     DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    INDEX idx_boss_loot_log_boss (boss_id)
) ENGINE=InnoDB;

-- ============================================================================
-- honor_events : event honor grants (x multiplier applied)
-- ============================================================================
CREATE TABLE IF NOT EXISTS honor_events (
    id                BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    event_id          VARCHAR(64)  NULL,
    event_type        VARCHAR(64)  NOT NULL,
    account_id        BIGINT       NOT NULL,
    base_honor        BIGINT       NOT NULL,
    multiplied_honor  BIGINT       NOT NULL,
    created_at        DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    INDEX idx_honor_events_account (account_id)
) ENGINE=InnoDB;

-- ============================================================================
-- account_honor : per-account honor balance
-- ============================================================================
CREATE TABLE IF NOT EXISTS account_honor (
    account_id  BIGINT      NOT NULL PRIMARY KEY,
    honor       BIGINT      NOT NULL DEFAULT 0,
    updated_at  DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
) ENGINE=InnoDB;

-- ============================================================================
-- skill_point_tomes : usage ledger for the custom Skill Point Tome
-- ============================================================================
CREATE TABLE IF NOT EXISTS skill_point_tomes (
    account_id                 BIGINT  NOT NULL PRIMARY KEY,
    tomes_used                 INT     NOT NULL DEFAULT 0,
    total_skill_points_gained  INT     NOT NULL DEFAULT 0
) ENGINE=InnoDB;

-- ============================================================================
-- character_skill_points : skill points granted via tomes (game server consumes)
-- ============================================================================
CREATE TABLE IF NOT EXISTS character_skill_points (
    character_id  BIGINT  NOT NULL PRIMARY KEY,
    points        INT     NOT NULL DEFAULT 0
) ENGINE=InnoDB;

-- ============================================================================
-- honor_shop_prices : original honor shop prices (divided by divisor at query)
-- ============================================================================
CREATE TABLE IF NOT EXISTS honor_shop_prices (
    item_id          BIGINT  NOT NULL PRIMARY KEY,
    original_price   BIGINT  NOT NULL
) ENGINE=InnoDB;

-- ============================================================================
-- character_combat_stats : universal Attack Power / Defense Power
-- ============================================================================
CREATE TABLE IF NOT EXISTS character_combat_stats (
    character_id   BIGINT  NOT NULL PRIMARY KEY,
    attack_power   BIGINT  NOT NULL DEFAULT 0,
    defense_power  BIGINT  NOT NULL DEFAULT 0
) ENGINE=InnoDB;

-- ============================================================================
-- vehicle_stats : carts / haulers
-- ============================================================================
CREATE TABLE IF NOT EXISTS vehicle_stats (
    vehicle_id       BIGINT   NOT NULL PRIMARY KEY,
    base_speed       FLOAT    NOT NULL DEFAULT 30.0,
    requires_fuel    BOOLEAN  NOT NULL DEFAULT FALSE,
    requires_grease  BOOLEAN  NOT NULL DEFAULT FALSE
) ENGINE=InnoDB;

-- ============================================================================
-- mount_stats : mounts (dragon type flagged for 30 m/s base)
-- ============================================================================
CREATE TABLE IF NOT EXISTS mount_stats (
    mount_id        BIGINT   NOT NULL PRIMARY KEY,
    base_speed      FLOAT    NOT NULL DEFAULT 21.0,
    is_dragon_type  BOOLEAN  NOT NULL DEFAULT FALSE
) ENGINE=InnoDB;