# Compatibility and evidence tiers

## Pinned baseline

| Component | Revision |
|---|---|
| PythonModule | `cce51ea9963ef1d4bd724a879f75dd343bb31368` |
| SerialEM | `1e18695024f5167edd37468899a3b3fc0bc11691` |
| MacroMasterList blob | `f156fbe3ed926753e783b1bc74f3cdac230a48d3` |
| Local header SHA256 | `33fbbb77a65d31d17dcbefaf1b0ae9f1562fa409f635b27d42b3191c61f05afe` |

## Coverage matrix

`docs/api-matrix.csv` is generated from the pinned header and includes:

- 843 complete CME enum rows, including 50 enum-only rows retained to preserve numbering;
- 793 external wrapper rows, all mapped to generated Rust methods;
- 10 special PythonModule surfaces, mapped to client/image/error APIs.

The reconciliation check currently reports 793 manifest names, 793 generated wrapper names, zero missing, and zero extra. `report_num_module_funcs()` intentionally returns the pinned PythonModule value 843 (`CME_ENUM_LENGTH`); `report_num_external_funcs()` returns 793 for the externally generated wrapper count.

## Evidence tiers

### Static-confirmed

- command ordering and argument keys from the pinned `MacroMasterList.h`;
- operation codes and client state behavior from the pinned PythonModule sources;
- endpoint, chunk constants, MRC modes, and error constants from the pinned C++ headers/sources.

### Locally verified

- little-endian frame and item-array encoding/decoding;
- partial TCP frame reads, read-ahead, one early reconnect, timeout behavior;
- regular numeric/string/tuple results and STOP mapping;
- image metadata, chunk handshake, raw transfer, buffer validation, and malformed input handling;
- generated API counts and manifest reconciliation;
- `cargo fmt`, `cargo check`, `cargo test`, and `cargo clippy` gates;
- zero-argument frame golden coverage, compact negative-error frames for regular/handshake/image paths, image allocation/metadata safety, direct-to-final-buffer chunk reads, 16,810,000-byte image sub-chunk planning/transfer, connection invalidation, re-handshake after reconnect, fixed-response payload rejection, per-chunk >1024-byte image timeout behavior, absolute command deadlines, image-write deadlines, retry-window separation, and default no-retry behavior.
- configuration rejection for invalid signed-32-bit frame/image limits, zero command/image deadlines, and excessive chunk counts; per-chunk read timeout restoration for mixed 1025/1024-byte transfers; deterministic absolute raw-write deadline coverage across multiple writes; low-level raw transport methods are crate-visible only; connection-loss classification and non-ASCII UTF-8 golden coverage.

### Live-blocked / not yet verified

- an actual running SerialEM server;
- command behavior against a real SerialEM macro processor;
- camera, stage, microscope, navigator, plugin, and hardware side effects;
- full-scale image transfers on the target installation;
- version drift after the pinned upstream revisions;
- live network authentication/encryption (the protocol provides neither);
- command cancellation tokens (blocking API only; configure absolute command/image deadlines instead).

## Version drift policy

Do not update `MacroMasterList.h` in place. For a new upstream revision, add a new provenance record or branch, regenerate the matrix, compare every command ID, minimum argument, flags, and key string, rerun mock/golden tests, and record the target SerialEM version before using it with hardware. There is no runtime version negotiation to compensate for command-table drift. A transport reconnect invalidates the client handshake state; the next regular command must perform `PSS_OKtoRunExternalScript` again.

## Licensing note

The pinned PythonModule tree has no `LICENSE` file, but its pinned `ReadMe.txt` contains the MIT notice and is now reproduced in `vendor/serialem/ReadMe.txt`. The vendored SerialEM `Copyright.txt` is likewise reproduced and records the MIT source license plus separate plugin/third-party boundaries. Redistribution of the Windows ICO assets remains unresolved. The vendored header is retained for reproducible command-ID generation, and generator verification now checks its recorded Git blob SHA-1, SHA-256, and input filename before generation. The initial implementation commit is `e6e166d`; release preparation still requires a reviewed provenance audit. The default image limit is a conservative 256 MiB policy and is not an upstream protocol guarantee. `.github/workflows/ci.yml` runs format, generation-drift, check, test, clippy, and whitespace gates on Ubuntu and Windows.
