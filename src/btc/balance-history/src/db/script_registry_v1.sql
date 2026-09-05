CREATE TABLE meta (
    id                      INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    schema_version          TEXT NOT NULL,
    policy                  TEXT NOT NULL,
    btc_network             TEXT NOT NULL,
    btc_genesis_hash        TEXT NOT NULL,
    base_height             INTEGER NOT NULL,
    base_block_hash         TEXT NOT NULL,
    core_snapshot_id        TEXT NOT NULL,
    entry_count             INTEGER NOT NULL,
    generated_at            INTEGER NOT NULL
);

CREATE TABLE script_registry (
    script_hash     BLOB NOT NULL PRIMARY KEY CHECK (length(script_hash) = 32),
    script_pubkey   BLOB NOT NULL
) WITHOUT ROWID;
