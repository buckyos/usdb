CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    block_height    INTEGER NOT NULL,
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
    outpoint       BLOB    NOT NULL PRIMARY KEY, -- 36 bytes
    script_hash    BLOB    NOT NULL,              -- 32 bytes
    value          INTEGER NOT NULL,              -- u64
);