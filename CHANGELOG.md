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
