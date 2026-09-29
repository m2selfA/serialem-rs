# serialem-client

`serialem-client` is an unofficial, blocking Rust client for the SerialEM external-script socket protocol. The repository is named `serialem-rs`. Publishing is currently disabled until the Rust client license, canonical repository URL, and Windows icon redistribution status are reviewed; see `LICENSE-STATUS.md` and `docs/release-checklist.md`.

It is generated and tested against these pinned upstream trees:

- PythonModule: `cce51ea9963ef1d4bd724a879f75dd343bb31368`
- SerialEM: `1e18695024f5167edd37468899a3b3fc0bc11691`

## Current evidence status

- 843 SerialEM command IDs are preserved.
- 793 PythonModule command wrappers are generated.
- `report_num_module_funcs()` returns 843 to match the pinned upstream PythonModule; `report_num_external_funcs()` returns the 793 generated wrapper count.
- Protocol, transport, image chunking, typed results, timeout, malformed-frame, connection invalidation, retry-policy and mock-server tests pass.
- No live SerialEM/electron-optics integration has been run yet. Do not treat this crate as hardware-validated.
- The pinned PythonModule tree has no separate `LICENSE` file; its pinned `ReadMe.txt` license notice is reproduced in `vendor/serialem/ReadMe.txt`. Redistribution boundaries remain tracked in `vendor/serialem/provenance.toml` and `LICENSE-STATUS.md`.

## Basic usage

```rust
use serialem_client::{CommandResult, SerialEmClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut serialem = SerialEmClient::connect_default()?;
    let result = serialem.ReportMag()?;
    assert!(matches!(result, CommandResult::Number(_)));
    Ok(())
}
```

`connect_default()` uses `127.0.0.1:48888` by default and honors the PythonModule-compatible environment variables `PY_SERIALEM_IP` and `PY_SERIALEM_PORT`. Use `connect_at` for an explicit endpoint.

`TransportConfig::default()` does **not** automatically retry a request after a lost response, because SerialEM may already have executed a side-effecting command. The normal `send_frame()` path never retries. The lower-level `TcpTransport::send_frame_idempotent()` method is opt-in and must only be used for requests whose effects are known to be idempotent. Its `retry_window` only controls whether a second request may be attempted after a transport failure; it does not limit a healthy first request. `TransportConfig::command_timeout` is an absolute deadline for the complete framed exchange, not a per-read timeout. `TransportConfig::image_write_timeout` optionally bounds the complete raw image upload after the metadata ACK; `read_timeout` and `write_timeout` remain unlimited by default for long microscope operations. The default image allocation limit is a conservative 256 MiB process-safety policy; callers can change it through `TransportConfig::max_image_bytes`. Use `close()` for permanent shutdown, `invalidate()` for recoverable failure, and `reconnect()` for explicit recovery. `Error::is_timeout()` distinguishes an I/O deadline from `Error::is_connection_lost()`; transport internals use `is_transport_failure()`.

SerialEM must be configured to allow external Python/script control. The Rust client does not bypass SerialEM's readiness or STOP handling. `mark_script_initialized()` is an advanced escape hatch that bypasses readiness checks; normal applications should not use it.

The generated wrapper surface includes many hardware-mutating commands (stage motion, magnification, acquisition, camera and file operations). Start with read-only commands such as `ReportMag` when validating a connection. `PutImageOptions::new(mode, width, height)` provides builder-compatible defaults for buffer/capture parameters when matching PythonModule-style calls.

Windows icon assets are vendored under `assets/windows/`: `SerialEM.ico` is reserved for future executables, the primary business window, and the primary tray icon; `SerialEMDoc.ico` is reserved for documentation/help-related Windows surfaces. They are not embedded by this library crate yet. See `assets/windows/README.md` for provenance and licensing boundaries.

The socket protocol has no authentication or encryption. Framed lengths and image byte counts are limited to signed 32-bit protocol values, and server-declared image chunks are capped defensively at one million. Keep the endpoint on localhost or a controlled private network and apply firewall rules before setting `PY_SERIALEM_IP` to a remote address. Configure `command_timeout` for a complete framed-command deadline when operating across unreliable networks; the blocking API has no cancellation token. String fields are transmitted as NUL-terminated UTF-8 bytes by this crate; invalid UTF-8 responses are rejected.

## Generated API

The command surface is generated from `vendor/serialem/MacroMasterList.h`; generated metadata also records each command's CME ID, minimum arguments, flags, and argument keys:

```powershell
cargo run --bin generate_api -- vendor/serialem/MacroMasterList.h docs/api-matrix.csv docs/api-matrix.md src/generated/command_ids.rs src/generated/commands.rs vendor/serialem/provenance.toml
cargo fmt --all
```

The generator validates 843 enum entries and 793 external wrappers, then runs `rustfmt` on generated Rust files.

## Validation

```powershell
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo test --all-targets --all-features
cargo test --doc --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo package --locked
```

CI is defined in `.github/workflows/ci.yml` for Ubuntu and Windows; it also regenerates the pinned API and rejects generated-file drift.

The live SerialEM test is deliberately ignored and requires a running, configured SerialEM instance:

```powershell
cargo test --test integration_serialem -- --ignored --nocapture
```
