CREATE TABLE meta (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    state_json TEXT NOT NULL
);
CREATE TABLE balances (
    script_hash BLOB NOT NULL PRIMARY KEY CHECK (typeof(script_hash) = 'blob' AND length(script_hash) = 32),
    balance INTEGER NOT NULL CHECK (typeof(balance) = 'integer' AND balance > 0)
) WITHOUT ROWID;
CREATE TABLE utxos (
    outpoint BLOB NOT NULL PRIMARY KEY CHECK (typeof(outpoint) = 'blob' AND length(outpoint) = 36),
    script_hash BLOB NOT NULL CHECK (typeof(script_hash) = 'blob' AND length(script_hash) = 32),
    value INTEGER NOT NULL CHECK (typeof(value) = 'integer' AND value >= 0)
) WITHOUT ROWID;
CREATE TABLE block_commits (
    block_height INTEGER NOT NULL PRIMARY KEY CHECK (block_height > 0 AND block_height < 4294967295),
    btc_block_hash BLOB NOT NULL CHECK (typeof(btc_block_hash) = 'blob' AND length(btc_block_hash) = 32),
    balance_delta_root BLOB NOT NULL CHECK (typeof(balance_delta_root) = 'blob' AND length(balance_delta_root) = 32),
    block_commit BLOB NOT NULL CHECK (typeof(block_commit) = 'blob' AND length(block_commit) = 32)
);
CREATE TABLE script_registry (
    script_hash BLOB NOT NULL PRIMARY KEY CHECK (typeof(script_hash) = 'blob' AND length(script_hash) = 32),
    script_pubkey BLOB NOT NULL CHECK (typeof(script_pubkey) = 'blob')
) WITHOUT ROWID;
CREATE TABLE genesis_block (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    raw_block BLOB NOT NULL CHECK (typeof(raw_block) = 'blob')
);
