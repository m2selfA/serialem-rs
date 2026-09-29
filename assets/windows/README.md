# Windows icon resources

These ICO files are preserved byte-for-byte from the local SerialEM icon store:

- `SerialEM.ico` — `D:\opt\scoop\persist\clew\icons\SerialEM.ico`
- `SerialEMDoc.ico` — `D:\opt\scoop\persist\clew\icons\SerialEMDoc.ico`

Current SHA-256 for both source and vendored files:

```text
C976980964FEF8509A2ED17A61AA50DC56827265620CA44C99952AFB0BCEA313
```

## Planned Windows use

- `SerialEM.ico`: default icon for future Windows executables, the primary business window, and the primary system-tray menu/icon.
- `SerialEMDoc.ico`: icon for future documentation/help-related executables, windows, and tray menu entries.

This crate is currently a library and does not yet embed these resources into an executable or GUI. A future Windows binary can use `SerialEM.ico` through its PE resource/build configuration; a GUI/tray frontend should load the appropriate ICO at runtime or embed it through the selected framework's resource mechanism.

## Provenance boundary

The files are vendored as requested for project continuity. Their upstream redistribution/license status has not been independently established; do not treat this directory as a license grant.
