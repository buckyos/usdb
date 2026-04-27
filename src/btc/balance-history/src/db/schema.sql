CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    block_height    INTEGER NOT NULL,
    balance_history_count INTEGER NOT NULL,
    utxo_count      INTEGER NOT NULL,
    block_commit_count INTEGER NOT NULL,
    script_registry_count INTEGER NOT NULL DEFAULT 0,
    generated_at    INTEGER NOT NULL,
    version         INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS balance_history (
    script_hash BLOB    NOT NULL PRIMARY KEY, -- 32 bytes
    height      INTEGER NOT NULL,    -- u32
    balance     INTEGER NOT NULL,    -- u64
    delta       INTEGER NOT NULL     -- i64
);

CREATE TABLE IF NOT EXISTS utxos (
    outpoint       BLOB    NOT NULL PRIMARY KEY,  -- 36 bytes
    script_hash    BLOB    NOT NULL,              -- 32 bytes
    value          INTEGER NOT NULL               -- u64
);

CREATE TABLE IF NOT EXISTS block_commits (
    block_height       INTEGER NOT NULL PRIMARY KEY,
    btc_block_hash     BLOB    NOT NULL,          -- 32 bytes
    balance_delta_root BLOB    NOT NULL,          -- 32 bytes
    block_commit       BLOB    NOT NULL           -- 32 bytes
);

CREATE TABLE IF NOT EXISTS script_registry (
    script_hash     BLOB    NOT NULL PRIMARY KEY, -- 32 bytes
    script_pubkey   BLOB    NOT NULL              -- raw BTC locking script
);
