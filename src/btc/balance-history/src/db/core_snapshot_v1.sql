CREATE TABLE meta (
    id                      INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    block_height            INTEGER NOT NULL,
    balance_history_count   INTEGER NOT NULL,
    utxo_count              INTEGER NOT NULL,
    block_commit_count      INTEGER NOT NULL,
    generated_at            INTEGER NOT NULL,
    schema_version          TEXT NOT NULL,
    db_identity_json        TEXT NOT NULL,
    core_snapshot_id        TEXT NOT NULL
);

CREATE TABLE balance_history (
    script_hash BLOB    NOT NULL PRIMARY KEY,
    height      INTEGER NOT NULL,
    balance     INTEGER NOT NULL,
    delta       INTEGER NOT NULL
);

CREATE TABLE utxos (
    outpoint       BLOB    NOT NULL PRIMARY KEY,
    script_hash    BLOB    NOT NULL,
    value          INTEGER NOT NULL
);

CREATE TABLE block_commits (
    block_height       INTEGER NOT NULL PRIMARY KEY,
    btc_block_hash     BLOB    NOT NULL,
    balance_delta_root BLOB    NOT NULL,
    block_commit       BLOB    NOT NULL
);
