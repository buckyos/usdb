# USDB Change Fragments

Every operator-visible or protocol-visible change must add one immutable JSON
fragment under `.release-notes/fragments/`. The same path is used in the USDB,
go-ethereum, and SourceDAO repositories. The cross-repository release workflow
reads fragments from the exact revisions frozen by the release manifest.

Use a globally unique, lowercase kebab-case `change_id`; the file name must be
`<change_id>.json`. A change spanning repositories should have one owning
fragment with all affected scopes instead of duplicate fragments in each
repository.

```json
{
  "schema_version": "usdb-change-fragment:v1",
  "change_id": "example-change",
  "type": "changed",
  "scopes": ["deployment", "node-runtime"],
  "summary": "Describe the result in one line",
  "details": [
    "Describe observable behavior and important implementation boundaries.",
    "Describe failure, recovery, or review consequences when relevant."
  ],
  "operator_actions": [
    "Run the explicit operator action before restarting the node."
  ],
  "compatibility": {
    "network_reset": false,
    "data_rebuild": false,
    "config_change": true,
    "restart_required": true
  },
  "references": [
    "https://github.com/buckyos/usdb/issues/1"
  ]
}
```

Allowed `type` values are `added`, `changed`, `deprecated`, `fixed`,
`internal`, `removed`, and `security`. Allowed scopes are defined by
`docker/scripts/tools/release_notes.py`; extend that reviewed list before using
a new scope.

Compatibility fields have operational meanings:

- `network_reset`: existing nodes cannot remain on the same network identity;
- `data_rebuild`: the network remains the same, but derived/local data must be rebuilt;
- `config_change`: an operator-owned setting must change;
- `restart_required`: the installed runtime must be replaced or restarted.

Set only the strongest required data action: `network_reset=true` makes
`data_rebuild=true` redundant. `operator_actions` may be empty when the release
requires no release-specific manual action.

Fragments are append-only after publication. Never edit or delete a fragment
already included in a GitHub Release. The generator rejects such mutations
between release revisions.

Commit bodies may include one trailer:

```text
Release-Note: example-change
```

Use `Release-Note: none` for maintenance commits that intentionally have no
release-note entry. Schema v1 reports missing or unknown trailers in
`release-changes.json` but does not block Candidate generation. Reviewers must
inspect the unclassified list; a later schema may make full coverage mandatory.

Validate current fragments locally with:

```bash
python3 docker/scripts/tools/release_notes.py validate-fragments \
  --repository-root .
```
