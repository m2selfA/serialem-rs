# Release checklist

This crate is not release-ready until each item is evidenced.

## Required gates

- [x] `cargo fmt --all -- --check`
- [x] `cargo check --all-targets --all-features`
- [x] `cargo test --all-targets --all-features`
- [x] `cargo clippy --all-targets --all-features -- -D warnings`
- [x] `cargo test --doc --all-features`
- [x] `cargo package --locked`
- [x] CI package-content audit includes provenance files and Windows icons
- [x] Generator verifies `MacroMasterList.h` path, Git blob SHA-1, and SHA-256
- [x] Absolute framed-command deadline and retry-window tests
- [x] PythonModule-compatible 16,810,000-byte raw sub-chunk planning/transfer tests
- [x] Transport configuration validation and connection-loss classification
- [x] Per-chunk image read timeout semantics and configurable image write deadline
- [x] Retry reconnect deadline clamping and pre-allocation encoder limits
- [x] Generator duplicate-key/multiline/provenance-tamper regression tests
- [x] API generator runs idempotently and generated files are clean
- [x] GitHub Actions workflow exists at `.github/workflows/ci.yml`
- [x] Live integration test is present and explicitly ignored by default
- [ ] Run the ignored live integration test against the target SerialEM build
- [ ] Record the SerialEM build/version, configuration, endpoint, and result
- [ ] Restore/verify the clean baseline after live testing
- [x] Create the initial Git commit (`e6e166d`); review remains part of release approval
- [x] Record pinned PythonModule MIT notice in `vendor/serialem/ReadMe.txt`
- [x] Record pinned SerialEM MIT/source notice in `vendor/serialem/Copyright.txt`
- [ ] Obtain and record redistribution permission/license for the vendored Windows icon resources
- [ ] Select a license for the Rust client and establish the canonical serialem-rs repository URL

## Provenance boundary

The vendored `MacroMasterList.h` is pinned for reproducible command-ID generation. The pinned PythonModule tree has no separate `LICENSE` file, but its `ReadMe.txt` MIT notice is reproduced in `vendor/serialem/ReadMe.txt`; SerialEM source terms and third-party/plugin boundaries are reproduced in `vendor/serialem/Copyright.txt`. These notices document provenance, not an independent redistribution approval. Windows icon permission and the Rust client license remain explicit release blockers in `vendor/serialem/provenance.toml` and `LICENSE-STATUS.md`.

## Retry, timeout, and memory policy

- Normal client commands never retry after a lost response.
- `TcpTransport::send_frame_idempotent` is only for requests whose effects are proven idempotent.
- `TransportConfig::command_timeout` can bound framed exchanges; read/write timeouts are otherwise unlimited by default for long operations.
- The default image limit is a conservative 256 MiB policy. Raise it explicitly only after assessing the target process and image workload.
- The protocol has no authentication or encryption. Remote `PY_SERIALEM_IP` use requires localhost/private-network controls and firewall review.
