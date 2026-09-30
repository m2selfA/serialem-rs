# Changelog

## Unreleased

- Repository maintenance synchronized README/release provenance wording with the vendored notices and expanded the documented validation commands (`cargo test --doc`, `cargo package --locked`).
- Added pinned PythonModule/SerialEM provenance and generated API matrix.
- Added blocking TCP protocol, regular command transport, reconnect, timeout, and image chunk support.
- Generated all 843 command IDs and 793 external wrapper methods.
- Added protocol, transport, mock-server, typed-result, image, malformed-frame, timeout, connection-invalidation, zero-argument golden, and manifest metadata tests.
- Disabled automatic retry by default; added explicit `send_frame_idempotent()` semantics.
- Re-handshake after reconnect, direct-to-final-buffer image reads, timeout overflow protection, strict response lengths, outgoing frame limits, and chained error sources.
- Added ignored live integration scaffold; no real SerialEM session has been verified.
- Clarified upstream `ReportNumModuleFuncs` (843 CME entries) versus external wrappers (793).
- Added absolute command deadlines, retry-deadline checks, PythonModule-compatible image sub-chunk writes, transport configuration validation, connection-loss classification, and UTF-8 golden coverage.
- Added byte-preserved Windows icon resources under `assets/windows/` for future executables, business windows, and tray surfaces.
- Added strict fixed-response decoding, reconnect lifecycle APIs, command deadlines, upstream-compatible image timeout thresholds, provenance-driven generation, quote-aware manifest checks, and read-only safety examples.
- Live SerialEM integration remains intentionally unverified.

### Verified against the official PythonModule (2026-10-01)

- Added `Error::kind_name()` for stable error-kind labels used by external harnesses, with a stability test covering every variant.
- Cross-checked the installed SerialEM 4.2.28 `serialem.cp313-win_amd64.pyd` function surface against the generated Rust surface in an isolated uv harness: 801 Python exports vs 574 Rust wrappers, 572 common; only `AddToFrameStackMdoc`/`StartFrameStackMdoc` are Rust-only (the module ships the renamed `AddToNextFrameStackMdoc`), which also explains the 841-vs-843 `ReportNumModuleFuncs` delta. The Rust command table is substantively compatible with the installed module.
- Confirmed on a dummy instance that `OKtoRunExternalScript` stays false and regular commands hang identically for both clients (dummy macro-processor idle-loop limitation, not a client bug); deeper live parity remains out of scope until an interactive or non-dummy session is available.
