# SerialEM socket protocol

This document records the protocol boundary implemented by this crate. The PythonModule C++ client is the behavioral reference; `Docs/socketProtocol.txt` in SerialEM is supplementary evidence.

## Endpoint

- Default: `127.0.0.1:48888`
- Overrides: `PY_SERIALEM_IP`, `PY_SERIALEM_PORT`
- Transport: TCP stream, one synchronous command at a time
- Security: no authentication or encryption; use localhost or a controlled private network with firewall restrictions
- Wire assumptions: little-endian Windows `LONG`/`BOOL` (4 bytes), `double` (8 bytes)

## Frame layout

Every framed exchange starts with a 4-byte total byte count, including the count itself. The operation-specific fields follow as fixed-width LONG, BOOL, double, or long-array data. String regions are NUL-terminated raw bytes; this crate encodes and decodes them as UTF-8 (`SERIAL_EM_STRING_ENCODING`), rejecting invalid UTF-8 on decode.

The crate uses explicit `i32`, `u32`, and `f64` rather than Rust platform-sized types. The implementation is intentionally aligned with the Windows SerialEM/PythonModule deployment rather than claiming a portable wire format.

## Operations

| Code | Operation | Rust implementation |
|---:|---|---|
| 1 | `PSS_RegularCommand` | `protocol::encode_regular_command`, `decode_regular_response` |
| 2 | `PSS_ChunkHandshake` | receive-side image chunk loop |
| 3 | `PSS_OKtoRunExternalScript` | `SerialEmClient::ok_to_run_external_script` |
| 4 | `PSS_GetBufferImage` | `SerialEmClient::get_buffer_image` / `buffer_image` |
| 5 | `PSS_PutImageInbuffer` | `SerialEmClient::put_image_in_buffer` |

## Regular commands

The PythonModule reserves item slot zero. A command with `n` supplied values serializes item slots `0..=n`, and `last_non_empty_index` is the last supplied slot. Each item has an integer region, a double region, and a NUL-terminated string region. A zero-argument command is a special wire case: `AddLongsAndStrings(NULL, 0, NULL, 0)` still sends long-array length `1` followed by one zero LONG word; it is not a zero-length array.

The Rust API preserves the dynamic result model through `CommandResult`:

- `None`
- `Number(f64)`
- `Text(String)`
- `Tuple(Vec<ReportValue>)`

`return_all_values_as_tuples(true)` changes a single numeric report into a one-element tuple, matching the PythonModule behavior. A single string remains scalar.

## Errors and exits

- `-9`: SerialEM busy/disconnected condition
- `-10`: user STOP
- `SCRIPT_NORMAL_EXIT`: normal script exit
- `SCRIPT_USER_STOP`: user STOP signaled by the command processor
- `SCRIPT_EXIT_NO_EXC`: exit without raising a script exception
- `Error::is_timeout()` identifies `TimedOut`/`WouldBlock` without claiming the peer disconnected.
- `Error::is_connection_lost()` classifies `Closed`, EOF, reset, and broken-pipe failures without changing their original error/source chain.
- `Error::is_transport_failure()` combines timeout, connection-loss, and malformed-frame classification for transport internals.

Malformed lengths, truncated arrays, unterminated strings, unsupported MRC modes, invalid buffer specifications, unexpected payloads in fixed-format responses, and compact negative-error frames are decoded locally. SerialEM negative responses contain only one LONG (8 bytes total) and map to `Busy`, `UserStop`, or `SerialEm`; the client invalidates the connection after such a response. `ReportNumModuleFuncs` follows the pinned PythonModule and returns the complete CME enum count (843); `report_num_external_funcs` exposes the generated wrapper count (793).

## Image transfer

Regular buffers are `A`–`T`; FFT buffer specifications are `AF`–`HF`. Supported MRC modes are byte, short, float, unsigned short, and RGB. `BufferImage` preserves row bytes, dimensions, mode, chunk count, and raw bytes.

Image reads use `PSS_ChunkHandshake` before each chunk after the first. Image writes send metadata super-chunks capped at `PYTHONMODULE_SUPER_CHUNK_SIZE` (336,200,000 bytes), with each raw socket write capped at `PYTHONMODULE_CHUNK_SIZE` (16,810,000 bytes), matching the pinned PythonModule behavior. `PutImageOptions::new` and `with_*` methods provide defaults for buffer/base-buffer/binning/capture fields while dimensions remain explicit. `set_buffer_image_timeout` applies only when the currently read chunk exceeds 1024 bytes, matching the pinned PythonModule condition; a large image split into smaller chunks retains the normal transport timeout. The client validates row/stride/byte-count relationships, caps server-declared chunks at `MAX_IMAGE_CHUNKS` (1,000,000), and applies `TransportConfig::max_image_bytes` before allocation; transfer failures invalidate the socket rather than reusing a half-consumed connection. Raw chunks are read directly into the final image buffer through `read_exact_into`, avoiding a second full-chunk allocation.

## Compatibility boundary

This is not an independent hardware-control protocol. SerialEM remains the server and executes the command against its own macro processor and hardware controllers. The Rust client must use a matching SerialEM command table; generated IDs, flags, minimum arguments, and argument keys are pinned and audited through `vendor/serialem/provenance.toml`, `docs/api-matrix.csv`, and the generated command metadata. There is no runtime version-negotiation operation in this protocol. Normal framed sends never retry; `send_frame_idempotent` is an explicit lower-level escape hatch for requests proven idempotent, because a lost response is otherwise ambiguous. Its retry window only gates a second request after failure; it never limits a healthy first request. `close()` permanently closes a transport, `invalidate()` permits automatic recovery on the next send, and `reconnect()` is the explicit recovery operation. The lower-level raw read/write helpers are crate-visible only; callers should use `SerialEmClient` to preserve handshake and image sequencing.

The default read/write socket timeouts are unlimited to accommodate long microscope operations. `TransportConfig::command_timeout`, when configured, is an absolute deadline for one complete framed exchange: each read/write and reconnect receives only the remaining duration. `TransportConfig::image_write_timeout` can independently bound the complete raw image upload after metadata ACK; the same absolute deadline covers every raw write in the upload and timeout restoration invalidates the connection. Applications crossing unreliable networks should configure these deadlines; the blocking API has no built-in cancellation token.
