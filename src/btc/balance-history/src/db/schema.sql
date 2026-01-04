CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    block_height    INTEGER NOT NULL,
    generated_at    INTEGER NOT NULL,
    version         INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS balance_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    script_hash BLOB    NOT NULL,    -- 20 bytes
    height      INTEGER NOT NULL,
    balance     INTEGER NOT NULL,    -- u64
    delta       INTEGER NOT NULL,
    
    UNIQUE(script_hash, height)
);

CREATE INDEX IF NOT EXISTS idx_balance_history_script_hash 
    ON balance_history(script_hash);

CREATE INDEX IF NOT EXISTS idx_balance_history_height 
    ON balance_history(height);