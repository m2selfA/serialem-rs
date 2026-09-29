# SerialEM PythonModule API matrix

This file is generated from the pinned `vendor/serialem/MacroMasterList.h`.

## Baseline

- PythonModule commit: `cce51ea9963ef1d4bd724a879f75dd343bb31368`
- SerialEM commit: `1e18695024f5167edd37468899a3b3fc0bc11691`
- Complete CME enum entries: `843`
- External generated wrappers: `793`
- Enum-only/internal entries retained for numbering: `50`

The CSV contains one row for every enum entry plus the non-macro PythonModule surface.
`min_args` is the SerialEM macro-command minimum; `arg_keys` describes the external wrapper's typed argument slots and may differ when compatibility/default handling is applied.
Command IDs must be assigned using every enum row, including rows that do not expose an external wrapper.

## Special PythonModule surface

| Name | Category | Protocol | Notes |
|---|---|---|---|
| `ConnectToSEM` | function | `client` | Initializes the socket client; default external endpoint is 127.0.0.1:48888 |
| `ScriptIsInitialized` | function | `client` | Marks the client as an already initialized SerialEM script |
| `ReturnAllValuesAsTuples` | function | `local` | Controls whether regular command reports are always returned as tuples |
| `ReportNumModuleFuncs` | function | `local` | Reports the complete CME enum count (843 in the pinned baseline); external wrappers are tracked separately |
| `SetBufferImageTimeout` | function | `local` | Sets the receive timeout used for large image transfers |
| `PutImageInBuffer` | function | `PSS_PutImageInBuffer` | Sends raw image bytes and metadata to a SerialEM buffer |
| `bufferImage` | type | `PSS_GetBufferImage/PSS_ChunkHandshake` | Read-only image view with dimensions, row stride and format metadata |
| `SEMerror` | exception | `local` | SerialEM command error |
| `SEMmoduleError` | exception | `local` | Python module or protocol error |
| `SEMexited` | exception | `local` | SerialEM process exited or disconnected |

## Rust mapping

| Python surface | Rust mapping | Status |
|---|---|---|
| `ConnectToSEM` | `SerialEmClient::connect_default/connect_at` | implemented-core |
| `ScriptIsInitialized` | `SerialEmClient::mark_script_initialized` | implemented-core |
| `PutImageInBuffer` | `SerialEmClient::put_image_in_buffer` | implemented-core |
| `ReturnAllValuesAsTuples` | `SerialEmClient::return_all_values_as_tuples` | implemented-core |
| `ReportNumModuleFuncs` | `SerialEmClient::report_num_module_funcs (843 upstream CME count)` | implemented-core |
| `SetBufferImageTimeout` | `SerialEmClient::set_buffer_image_timeout` | implemented-core |
| `bufferImage` | `SerialEmClient::buffer_image + BufferImage` | implemented-core |
| `SEMerror` | `Error::SerialEm / Error::SerialEmMessage` | implemented-core |
| `SEMmoduleError` | `Error::InvalidArgument / Error::InvalidFrame` | implemented-core |
| `SEMexited` | `Error::ScriptExited / Error::UserStop` | implemented-core |

## Status

External command rows are `generated`; enum-only rows preserve numbering; special surfaces are `implemented-core`. Real SerialEM integration and per-command golden coverage remain pending.
