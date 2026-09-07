# Ord release dependencies

`ord-0.23.3.Cargo.lock` is the reviewed dependency resolution for the unchanged
Ord 0.23.3 source commit `ba60f87b530c01b15f6f8645e2ed4ef52f3f9f74`.
The services Dockerfile verifies that commit, replaces the upstream lockfile,
and builds with `cargo auditable build --locked`. This keeps dependency security
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
