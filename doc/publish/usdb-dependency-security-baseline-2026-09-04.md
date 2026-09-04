# USDB Dependency Security Baseline - 2026-09-04

## 1. Status

This is the first report-only dependency vulnerability baseline. It records
scanner output before remediation and does not imply that every finding is
reachable or accepted.

| Surface | Result |
| --- | --- |
| `go-ethereum` source with Go 1.26.0 | 99 unique advisories: 48 symbol-level, 19 package-level, and 32 module-level |
| `geth` binary built with Go 1.18.5 | 133 unique advisories: 72 symbol-level, 9 package-level, and 52 module-level |
| USDB canonical Rust workspace | 17 vulnerabilities; 5 unmaintained, 7 unsound, and 2 yanked warnings |
| SourceDAO npm runtime | 0 vulnerabilities |
| SourceDAO complete npm build chain | 11 total: 5 High and 6 Low |
| balance-history browser npm runtime | 0 vulnerabilities |
| balance-history browser complete tree | 6 total: 4 High, 1 Moderate, and 1 Low |
| USDB console npm runtime | 3 Moderate |
| USDB console complete tree | 10 total: 4 High, 4 Moderate, and 2 Low |
| USDB indexer browser npm runtime | 0 vulnerabilities |
| USDB indexer browser complete tree | 6 total: 4 High, 1 Moderate, and 1 Low |

Scanner inputs and versions:

- npm 11.6.2 against committed lockfile version 3 data;
- cargo-audit 0.22.2 and the current RustSec advisory database;
- full and `--omit=dev` npm reports were run independently;
- no project lockfile or manifest was modified by the scans.

## 2. Rust baseline

The canonical `src/btc/Cargo.lock` contains these vulnerability groups:

- `aws-lc-sys 0.32.3`: five 2026 certificate, signature-validation, and
  cryptographic advisories; patched releases begin at 0.38.0 or 0.39.0;
- `bytes 1.10.1`: integer overflow in `BytesMut::reserve`, patched in 1.11.1;
- `crossbeam-epoch 0.9.18`: invalid pointer dereference in pointer formatting,
  patched in 0.9.20;
- `h2 0.4.12`: unbounded empty DATA frames, patched in 0.4.16;
- `quick-xml 0.37.5`: memory-exhaustion and quadratic-time XML parsing issues,
  patched in 0.41.0;
- `quinn-proto 0.11.13`: endpoint denial of service and remote memory
  exhaustion, patched in 0.11.15;
- `rustls-webpki 0.103.8`: four certificate/CRL validation issues, patched in
  later 0.103 releases;
- `time 0.3.44`: stack-exhaustion denial of service, patched in 0.3.47.

The initial dependency-tree check confirms that `h2 0.4.12` and
`bytes 1.10.1` are present in the active Linux HTTP/RPC paths used by
balance-history, usdb-indexer, and usdb-control-plane. These two upgrades are
the first Rust remediation candidates.

Some entries, including `quinn-proto`, may be target- or feature-specific and
require a target-aware dependency-tree and reachability review before they are
classified.

The additional committed lockfiles produced these overlapping results during
the initial inventory:

| Lockfile | Vulnerabilities | Warnings |
| --- | ---: | --- |
| `src/btc/balance-history-cli/Cargo.lock` | 12 | 3 unmaintained, 1 unsound |
| `src/btc/balance-history/Cargo.lock` | 12 | 3 unmaintained, 1 unsound |
| `src/btc/usdb-indexer/Cargo.lock` | 14 | 1 unmaintained, 6 unsound, 2 yanked |
| `src/btc/usdb-util/Cargo.lock` | 10 | 1 unsound |

`cargo metadata` and the services image build both confirm that these member
lockfiles do not participate in a supported build. They were removed after the
baseline; future Cargo and Dependabot operations use only
`src/btc/Cargo.lock`.

## 3. Go baseline

`govulncheck` v1.7.0 was built with a checksum-verified Go 1.26.0 toolchain.
The source scan uses call-graph analysis. The release-artifact scan was run
separately against a `geth` binary built from the current tree with the
canonical Go 1.18.5 release toolchain.

The source scan confirms reachable dependency findings in old versions of
`golang.org/x/crypto`, `github.com/golang-jwt/jwt/v4`, and other packages. It
also shows that the compatibility Go 1.26.0 toolchain itself predates multiple
standard-library security fixes available in later Go 1.26 patch releases.

The binary scan reports 72 advisories at symbol level. Binary symbol presence
is less precise than source-call reachability, but the result includes a large
set of `net/http`, HTTP/2, `crypto/tls`, `crypto/x509`, URL parsing, and resource
exhaustion fixes released after Go 1.18.5. It also includes dependency fixes for
`github.com/golang-jwt/jwt/v4`, `github.com/gorilla/websocket`,
`golang.org/x/net`, and `golang.org/x/text`.

The canonical release toolchain is therefore an unresolved release risk. A
resettable `testnet-v0` may use it only under an explicit, expiring exception
with restricted network exposure, immutable build inputs, and a qualified exit
plan. It remains a public-mainnet release blocker. Mainnet resolution requires
either moving the release build to a supported Go patch line and proving
compatibility, or maintaining and validating an explicit security-backport
toolchain. Merely updating the analysis toolchain does not remove
vulnerabilities compiled into the released node binary.

## 4. npm baseline

### SourceDAO

The production dependency projection reports no known vulnerabilities. The
complete contract build chain reports High findings through:

- direct `hardhat`;
- transitive `adm-zip`;
- transitive `brace-expansion`;
- transitive `js-yaml`;
- transitive `undici`.

These are not deploy-time runtime dependencies, but they process contract,
configuration, network, and artifact inputs during trusted builds. They must be
treated as release-chain findings rather than dismissed as ordinary test-only
dependencies.

### USDB web applications

All three complete trees report High findings in the Vite build chain through
`browserslist`, `nanoid`, `postcss`, and direct `vite`. The proposed Vite fix is
a major update and therefore requires a normal frontend build and behavior
review instead of a forced audit fix.

Only the console currently has runtime findings: three Moderate React Router
findings involving open redirects and related routing behavior. The affected
packages are direct `react-router-dom` and transitive `react-router` and
`@remix-run/router`; fixed versions are available.

## 5. Immediate triage order

1. Classify findings in the project-controlled Rust and SourceDAO surfaces,
   then resolve reachable Critical/High findings before broad inherited-code
   upgrades.
2. Upgrade or constrain `h2` and `bytes`, then rerun the Rust workspace tests
   and cargo-audit.
3. Resolve the console React Router runtime findings.
4. Review SourceDAO Hardhat and build-chain upgrade paths with bytecode golden
   and bootstrap tests.
5. Review Vite major-upgrade compatibility independently for each web app.
6. Classify the remaining Rust target-specific, unmaintained, unsound, and
   yanked warnings.
7. Move the Go compatibility lane to a supported patched toolchain, keep Go
   module versions fixed, and run the cross-version qualification matrix.
8. Promote the qualified Go toolchain before public mainnet; separately review
   inherited Go dependency upgrades afterward.
9. Add final-image scanning and SBOM generation before dependency findings
   become a release gate.

## 6. Enforcement decision

Do not enable a blanket failure threshold from these counts. The first gate
will reject newly introduced Critical or High findings after this baseline is
classified. Existing findings may remain only with an owner, remediation plan,
and expiry under `usdb-dependency-security-policy.md`.

The staged remediation and compatibility gates are defined in
`usdb-security-audit-and-toolchain-qualification-plan.md`.
