# USDB Dependency Security Policy

## 1. Purpose

This document defines how the USDB project inventories, scans, triages, and
updates third-party dependencies across the coordinated repositories:

- `buckyos/go-ethereum`;
- `buckyos/usdb`;
- `buckyos/SourceDAO`.

The initial rollout is intentionally report-only. Existing findings must be
classified before vulnerability severity is used as a merge or release gate.
The report-only phase must not be confused with accepting the findings.

## 2. Covered dependency surfaces

| Repository | Dependency surfaces |
| --- | --- |
| `go-ethereum` | Go modules, Go toolchain and standard library, GitHub Actions, release images |
| `usdb` | Rust crates, npm web applications, GitHub Actions, service and Bitcoin images |
| `SourceDAO` | npm runtime dependencies, Hardhat build dependencies, GitHub Actions, Solidity source |

Lockfiles are the primary inventory for language dependencies. The Rust
workspace has one canonical lockfile at `src/btc/Cargo.lock`; member crates must
not commit independent lockfiles. The final digest-pinned images remain an
independent release surface because they also contain operating-system packages
and compiled toolchains.

## 3. GitHub repository settings

Apply the following settings to all three public repositories.

### Enable immediately

- private vulnerability reporting;
- dependency graph;
- Dependabot alerts;
- Dependabot malware alerts;
- prevent direct Dependabot alert dismissals;
- secret scanning and push protection;
- CodeQL default setup for every supported language in the repository.

Keep Copilot Autofix enabled only as a suggestion source. Every proposed fix
still requires normal review and tests.

### Enable after `.github/dependabot.yml` is merged

- Dependabot security updates.

Security update pull requests must never be auto-merged. A dependency update
affecting consensus, block import, P2P, RPC, bootstrap, signing, storage, or
release construction requires the same review and tests as a direct code
change in that subsystem.

### Keep disabled during the baseline phase

- repository-wide grouped security updates;
- automatic dependency submission;
- direct CodeQL alert dismissal prevention;
- branch rules that fail on all existing security findings.

Repository-wide grouping obscures which update caused a regression. Add only
targeted groups in `dependabot.yml` after observing the initial update volume.
The checked-in Go, Cargo, and npm manifests already provide the first inventory;
automatic dependency submission should be reconsidered only when a build-time
dependency is missing from the dependency graph.

After the CodeQL baseline is classified, enable direct dismissal prevention
and require the stable CodeQL and dependency-security checks in the protected
branch ruleset.

## 4. Automated baseline

Each repository owns an independent `Dependency Security` workflow so a scan
does not depend on sibling worktrees or compatibility-lock freshness.

The workflows run when dependency manifests change, on manual dispatch, and on
a weekly schedule. Scanner versions and GitHub Actions are pinned. Reports are
retained as workflow artifacts for 30 days.

The first phase has these semantics:

- a valid report containing vulnerabilities does not fail the workflow;
- scanner installation, advisory database access, malformed output, or other
  failures do fail the workflow;
- the independent dependency-security workflow is not a required Fast,
  Nightly, Weekly, or ordinary feature-PR check during the baseline phase;
- npm produces both full and runtime-only reports;
- SourceDAO full results are release-relevant because Hardhat and bootstrap
  tooling participate in the contract build chain;
- Go uses reachability-aware source analysis and a separate scan of a `geth`
  binary built with the canonical release toolchain;
- Rust audits the canonical workspace lockfile with `cargo-audit`.

CodeQL complements dependency scanning. It does not replace `govulncheck`,
`cargo-audit`, npm advisory checks, final-image scanning, or Solidity-specific
analysis.

## 5. Finding classification

Every unresolved finding used in release review must record:

| Field | Meaning |
| --- | --- |
| `advisory_id` | Canonical GHSA, CVE, GO, RUSTSEC, or ecosystem advisory ID |
| `package` | Package and installed version |
| `fixed_version` | Smallest known fixed version, if available |
| `dependency_path` | Direct or transitive path from the project |
| `affected_artifact` | Binary, contract build, web application, image, or test tool |
| `reachability` | Confirmed reachable, imported only, not reachable, or unknown |
| `exposure` | Network input, local operator input, build input, or test-only input |
| `decision` | Upgrade, replace, remove, mitigate, or temporarily accept |
| `owner` | Person responsible for resolution |
| `expires_at` | Mandatory expiry for temporary acceptance |

Do not classify risk from the advisory count alone. One reachable consensus or
bootstrap issue can be more important than many unreachable development-tool
findings.

## 6. Priority and enforcement

Use the following initial policy:

- block a release for reachable Critical or High findings in runtime,
  consensus, P2P, RPC, signing, bootstrap, or release construction;
- treat Critical or High build-chain findings as release blockers unless the
  vulnerable behavior is demonstrably unreachable and an expiring exception is
  approved;
- assign an owner and deadline to reachable Medium findings;
- allow test-only or unreachable findings temporarily only with evidence and an
  expiry date;
- do not use permanent package-wide ignores;
- do not run forced or unreviewed major-version update commands.

Move from report-only to enforcement only after:

1. the first reports from all three repositories are archived;
2. every Critical and High finding is classified;
3. existing accepted findings have owners and expiry dates;
4. scanner infrastructure has completed successfully on at least two scheduled
   runs;
5. dependency update pull requests execute the normal repository test gates.

The first merge gate should reject only newly introduced Critical or High
findings, initially for dependency and security-related pull requests. Existing
classified findings remain governed by their owner and expiry instead of
blocking unrelated feature work. The public-mainnet release gate can then
enforce the stricter runtime, consensus, and build-chain policy. The detailed
rollout stages are defined in
`usdb-security-audit-and-toolchain-qualification-plan.md`.

## 7. Remediation workflow

1. Confirm the affected version and dependency path from the lockfile.
2. Confirm whether project code or a released binary reaches the vulnerable
   symbol or behavior.
3. Prefer the smallest supported fixed version.
4. Run subsystem tests and the coordinated USDB fast/integration gates.
5. Scan the updated lockfile again.
6. Record operator or compatibility impact in a release-note fragment only when
   the update has release-visible behavior; otherwise use `Release-Note: none`.
7. Close or dismiss the GitHub alert only with the remediation reference or an
   approved, expiring exception.

## 8. Planned second phase

The baseline does not yet cover every released byte. Before a public mainnet
release, add:

- digest-pinned image scanning for OS and language packages;
- SPDX or CycloneDX SBOM generation for every release image;
- release-manifest references to scan reports and SBOM digests;
- Solidity static analysis for SourceDAO;
- npm registry signature and provenance verification where supported;
- equivalent compiled-binary scans for any additional Go release commands.
