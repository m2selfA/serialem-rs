# License status

This file is intentionally **not** a license grant.

`serialem-client` currently vendors three provenance classes:

1. **Rust client code** — project licensing has not yet been selected.
2. **PythonModule / `MacroMasterList.h`** — the pinned upstream `ReadMe.txt` states an MIT license; the notice is reproduced in `vendor/serialem/ReadMe.txt`.
3. **SerialEM source/header provenance** — the pinned `Copyright.txt` states an MIT license for SerialEM source, while bundled plugins and third-party components retain separate terms; the notice is reproduced in `vendor/serialem/Copyright.txt`.
4. **Windows ICO assets** — provenance and redistribution permission are not established; see `assets/windows/README.md`.

The canonical repository is https://github.com/m2selfA/serialem-rs. The crate sets `publish = false` until the Rust client license and redistribution status of every vendored asset are reviewed. This status file exists to make `cargo package --locked` metadata explicit; it must not be interpreted as permission to redistribute any component.
