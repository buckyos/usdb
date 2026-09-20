# Ord release dependencies

## Current: Ord 0.29.0

The services image and the source-built world-sim image compile the official
`0.29.0` tag at `7e37a3bd3391044b39f5f11f20dfdb8b3764cd0e`, verify the commit and
binary version, and use `ord-0.29.0.Cargo.lock` with `--locked`. The services build
also embeds dependency metadata with `cargo-auditable 0.7.5`. Both use Rust 1.91.0.
No prebuilt third-party Ord image or unpinned latest binary is downloaded.

The lock starts from that exact upstream release. The 2026-09-20 review updates
`h2 0.4.15 -> 0.4.16` (RUSTSEC-2026-0258) and
`rustls 0.23.43 -> 0.23.45` (RUSTSEC-2026-0285), with required compatible updates
to `aws-lc-rs`, `aws-lc-sys` and `rustls-webpki`. `openssl 0.10.81` is already
present upstream. No Ord source or USDB embedded parser dependency is changed.
RustSec database commit `d5c17953a895cf19e8d3ce66eaa42b6fcfe1fb16` reports zero
vulnerabilities; `instant` and `net2` still have unmaintained warnings. This is a
dependency audit, not a final-image security qualification.

`ord-compatibility.yml` builds the actual release Docker stages and tests them
against Bitcoin Core 31.1 before the release services image is published. The
isolated test covers txindex, exact canonical height/hash, inscription creation,
content/address APIs, reorg removal/reconfirmation and restart persistence.
See [the local qualification record](../../doc/publish/ord-0.29.0-upgrade-validation.md).

The embedded `ord` Rust library used by USDB Indexer and the sibling
go-ethereum deterministic regtest toolchain have independent pins; they are not
changed by this external service upgrade.

## Archived: Ord 0.23.3

`ord-0.23.3.Cargo.lock` is the reviewed dependency resolution for the unchanged
Ord 0.23.3 source commit `ba60f87b530c01b15f6f8645e2ed4ef52f3f9f74`.
The previous services Dockerfile verified that commit, replaced the upstream lockfile,
and built with `cargo auditable build --locked`. This kept dependency security
updates independent from an Ord protocol/version upgrade.

The 2026-09-07 update changes compatible versions of `aws-lc-rs`, `bytes`,
`crossbeam-epoch`, `h2`, `rustls-webpki`, `time`, `rss`, `atom_syndication`,
and `openssl`/`openssl-sys`,
including their required transitive dependencies. The upstream lockfile had
14 RustSec vulnerability records; the updated lockfile has zero in the database
used for this review. Final-image Trivy additionally identified five CVEs in the
Rust OpenSSL bindings; `openssl 0.10.81` resolves those findings as well.
Unmaintained/unsound warnings remain separate findings.

For another update, start from the pinned Ord source, copy this lockfile into
that checkout, update only the reviewed dependencies, audit and build it, and
copy the resulting lockfile back here. Reassess the image exception source
fingerprint after reviewing any dependency changes. Do not run an unconstrained
dependency update inside the release Dockerfile.
