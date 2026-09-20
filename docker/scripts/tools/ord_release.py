"""Pinned external Ord release and its independent local dataset contract."""

VERSION = "0.29.0"
REVISION = "7e37a3bd3391044b39f5f11f20dfdb8b3764cd0e"
INDEX_SCHEMA = 34
LEGACY_VERSION = "0.23.3"
LEGACY_IDENTITY = {"schema_version": "usdb-ord-dataset:v1", "bitcoin_network": "main",
                   "ord_version": LEGACY_VERSION, "indexes": ["inscriptions", "addresses"]}
IDENTITY = {**LEGACY_IDENTITY, "ord_version": VERSION, "index_schema": INDEX_SCHEMA}
