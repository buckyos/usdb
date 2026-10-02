"""Validate frozen USDB rule selection separately from Bitcoin source identity."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
from typing import Mapping

SCOPE_ENV = "USDB_RULES_SCOPE"
REGISTRY_ENV = "BTC_ACTIVATION_REGISTRY_ID"
CATALOG_ENV = "BTC_ACTIVATION_REGISTRY_CATALOG_FILE"
CATALOG_ARTIFACT = "btc_activation_registry_catalog"
SELECTOR_ENV_KEYS = (SCOPE_ENV, REGISTRY_ENV, CATALOG_ENV)


def validate_scope(value: str) -> str:
    """Accept canonical scope names; legacy keeps its frozen pre-scope identity."""
    if (not isinstance(value, str) or not 1 <= len(value) <= 64
            or re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", value) is None):
        raise ValueError("USDB rules_scope must be a canonical lowercase scope name")
    return value


def indexer_rule_selection(env: Mapping[str, str]) -> dict:
    """Render only complete selectors; an external catalog is never discovered implicitly."""
    scope = env.get(SCOPE_ENV) or None
    registry_id = env.get(REGISTRY_ENV) or None
    catalog = env.get(CATALOG_ENV) or None
    if scope is not None:
        validate_scope(scope)
    if registry_id is not None and re.fullmatch(r"[0-9a-f]{64}", registry_id) is None:
        raise ValueError(f"{REGISTRY_ENV} must be lowercase SHA-256")
    if scope not in (None, "legacy") and (registry_id is None or catalog is None):
        raise ValueError("Scoped registry selection requires scope, registry ID and catalog file")
    if catalog is not None:
        if scope in (None, "legacy") or registry_id is None:
            raise ValueError("External registry catalog requires an explicit non-legacy scope and registry ID")
        path = Path(catalog)
        if not path.is_absolute() or not path.is_file():
            raise ValueError(f"{CATALOG_ENV} must be an existing absolute catalog file")
    return dict(rules_scope=scope, activation_registry_id=registry_id,
                activation_registry_catalog_file=catalog)


def frozen_rule_environment(network: dict) -> dict[str, str]:
    """Derive container selectors from the signed/frozen network artifact inventory."""
    source = network["btc_source"]
    scope = validate_scope(source.get("rules_scope", "legacy"))
    registry_id = source.get("activation_registry_id", "")
    if not isinstance(registry_id, str) or re.fullmatch(r"[0-9a-f]{64}", registry_id) is None:
        raise ValueError("Frozen BTC activation registry ID must be lowercase SHA-256")
    artifact = network.get("artifacts", {}).get(CATALOG_ARTIFACT)
    catalog_file = ""
    if artifact is not None:
        if scope == "legacy":
            raise ValueError("Legacy networks must use the frozen embedded registry catalog")
        if not isinstance(artifact, dict):
            raise ValueError("Frozen registry catalog artifact must be an object")
        relative_text = artifact.get("path", "")
        if not isinstance(relative_text, str):
            raise ValueError("Frozen registry catalog path must be a string")
        relative = PurePosixPath(relative_text)
        if (relative.as_posix() != relative_text or len(relative.parts) != 2
                or relative.parts[0] != "artifacts" or relative.suffix != ".json"
                or relative.name in {".", ".."}):
            raise ValueError("Registry catalog must be a JSON file directly below artifacts/")
        digest = artifact.get("sha256", "")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("Frozen registry catalog SHA-256 is required")
        catalog_file = str(PurePosixPath("/network") / relative.name)
    if scope != "legacy" and not catalog_file:
        raise ValueError("Scoped network requires a frozen registry catalog artifact")
    return {SCOPE_ENV: source.get("rules_scope", ""), REGISTRY_ENV: registry_id,
            CATALOG_ENV: catalog_file}


def validate_frozen_rule_selection(bundle_dir: Path, network: dict, env: Mapping[str, str]) -> None:
    """Reject release environment drift and changed catalog bytes before starting containers."""
    expected = frozen_rule_environment(network)
    for key, value in expected.items():
        if env.get(key, "") != value:
            raise ValueError(f"network.env frozen registry selector mismatch: {key}")
    artifact = network.get("artifacts", {}).get(CATALOG_ARTIFACT)
    if artifact is not None:
        path = bundle_dir / artifact["path"]
        if not path.is_file() or not path.resolve().is_relative_to(bundle_dir.resolve()):
            raise ValueError("Frozen registry catalog is missing or escapes the network bundle")
        if hashlib.sha256(path.read_bytes()).hexdigest() != artifact["sha256"]:
            raise ValueError("Frozen registry catalog SHA-256 mismatch")


def validate_node_rule_overrides(network: dict, env: Mapping[str, str]) -> None:
    """A private node configuration cannot switch the release's consensus selectors."""
    for key, expected in frozen_rule_environment(network).items():
        if key in env and env[key] != expected:
            raise ValueError(f"node.env cannot override frozen registry selector: {key}")


def rules_scope_identity(source: Mapping[str, object]) -> dict:
    """Preserve legacy contract bytes while separating explicitly scoped indexer datasets."""
    scope = validate_scope(source.get("rules_scope", "legacy"))
    return {} if scope == "legacy" else {"btc_rules_scope": scope}


if __name__ == "__main__":
    import os
    import sys
    try:
        print(json.dumps(indexer_rule_selection(os.environ), separators=(",", ":"))[1:-1])
    except ValueError as error:
        print(f"Indexer registry configuration rejected: {error}", file=sys.stderr)
        raise SystemExit(1)
