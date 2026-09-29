use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use sha1::Sha1;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
struct Provenance {
    pythonmodule_commit: String,
    serialem_commit: String,
    expected_enum_entries: usize,
    expected_external_wrappers: usize,
    file_path: String,
    repository_path: String,
    git_blob_sha: String,
    sha256: String,
}

#[derive(Debug, Clone)]
struct Entry {
    ordinal: usize,
    macro_kind: String,
    availability: String,
    macro_name: String,
    export_name: String,
    target: String,
    min_args: usize,
    flags: i32,
    arg_keys: String,
    cme_name: String,
}

const SPECIALS: &[(&str, &str, &str, &str)] = &[
    (
        "ConnectToSEM",
        "function",
        "client",
        "Initializes the socket client; default external endpoint is 127.0.0.1:48888",
    ),
    (
        "ScriptIsInitialized",
        "function",
        "client",
        "Marks the client as an already initialized SerialEM script",
    ),
    (
        "ReturnAllValuesAsTuples",
        "function",
        "local",
        "Controls whether regular command reports are always returned as tuples",
    ),
    (
        "ReportNumModuleFuncs",
        "function",
        "local",
        "Reports the complete CME enum count; external wrappers are tracked separately",
    ),
    (
        "SetBufferImageTimeout",
        "function",
        "local",
        "Sets the receive timeout used for large image transfers",
    ),
    (
        "PutImageInBuffer",
        "function",
        "PSS_PutImageInBuffer",
        "Sends raw image bytes and metadata to a SerialEM buffer",
    ),
    (
        "bufferImage",
        "type",
        "PSS_GetBufferImage/PSS_ChunkHandshake",
        "Read-only image view with dimensions, row stride and format metadata",
    ),
    ("SEMerror", "exception", "local", "SerialEM command error"),
    (
        "SEMmoduleError",
        "exception",
        "local",
        "Python module or protocol error",
    ),
    (
        "SEMexited",
        "exception",
        "local",
        "SerialEM process exited or disconnected",
    ),
];

const SPECIAL_MAPPINGS: &[(&str, &str, &str)] = &[
    (
        "ConnectToSEM",
        "SerialEmClient::connect_default/connect_at",
        "implemented-core",
    ),
    (
        "ScriptIsInitialized",
        "SerialEmClient::mark_script_initialized",
        "implemented-core",
    ),
    (
        "PutImageInBuffer",
        "SerialEmClient::put_image_in_buffer",
        "implemented-core",
    ),
    (
        "ReturnAllValuesAsTuples",
        "SerialEmClient::return_all_values_as_tuples",
        "implemented-core",
    ),
    (
        "ReportNumModuleFuncs",
        "SerialEmClient::report_num_module_funcs (upstream CME count)",
        "implemented-core",
    ),
    (
        "SetBufferImageTimeout",
        "SerialEmClient::set_buffer_image_timeout",
        "implemented-core",
    ),
    (
        "bufferImage",
        "SerialEmClient::buffer_image + BufferImage",
        "implemented-core",
    ),
    (
        "SEMerror",
        "Error::SerialEm / Error::SerialEmMessage",
        "implemented-core",
    ),
    (
        "SEMmoduleError",
        "Error::InvalidArgument / Error::InvalidFrame",
        "implemented-core",
    ),
    (
        "SEMexited",
        "Error::ScriptExited / Error::UserStop",
        "implemented-core",
    ),
];

fn split_macro_args(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut in_string = false;
    let chars: Vec<char> = input.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        match ch {
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string && depth > 0 => depth -= 1,
            ',' if !in_string && depth == 0 => {
                parts.push(
                    chars[start..index]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_string(),
                );
                start = index + 1;
            }
            _ => {}
        }
    }

    parts.push(chars[start..].iter().collect::<String>().trim().to_string());
    parts
}

fn parse_macro_line(line: &str, ordinal: usize) -> Result<Option<Entry>, String> {
    let line = line.trim();
    if !line.starts_with("MAC_") {
        return Ok(None);
    }

    let open = line
        .find('(')
        .ok_or_else(|| format!("missing '(' in macro line: {line}"))?;
    let close = line
        .rfind(')')
        .ok_or_else(|| format!("missing ')' in macro line: {line}"))?;
    if !line[close + 1..].trim().is_empty() {
        return Err(format!("unexpected text after macro line: {line}"));
    }

    let full_kind = &line[4..open];
    let (base_kind, external_suffix) = if let Some(base) = full_kind.strip_suffix("_NOARG") {
        (base, Some("NOARG"))
    } else if let Some(base) = full_kind.strip_suffix("_ARG") {
        (base, Some("ARG"))
    } else {
        (full_kind, None)
    };

    if !matches!(base_kind, "SAME_NAME" | "DIFF_NAME" | "SAME_FUNC") {
        return Err(format!("unknown macro kind {base_kind} in line: {line}"));
    }

    let args = split_macro_args(&line[open + 1..close]);
    if args.len() < 4 {
        return Err(format!("too few macro arguments in line: {line}"));
    }

    let min_args = args[1]
        .parse::<usize>()
        .map_err(|error| format!("invalid minimum argument count in {line}: {error}"))?;
    let flags = args[2]
        .parse::<i32>()
        .map_err(|error| format!("invalid flags in {line}: {error}"))?;
    let has_arg_keys = external_suffix == Some("ARG");
    let cme_index = args.len() - 1 - usize::from(has_arg_keys);
    let cme_name = args[cme_index].clone();
    if cme_name.is_empty() {
        return Err(format!("empty CME name in line: {line}"));
    }

    let target = if base_kind == "SAME_NAME" {
        String::new()
    } else {
        if args.len() < 5 {
            return Err(format!("missing target function in line: {line}"));
        }
        args[3].clone()
    };

    let arg_keys = if has_arg_keys {
        let keys = args
            .last()
            .expect("ARG macro must have an argument-key field")
            .clone();
        if !keys
            .chars()
            .all(|key| matches!(key, 'D' | 'S' | 'I' | 'd' | 's' | 'i'))
        {
            return Err(format!("invalid argument keys '{keys}' in line: {line}"));
        }
        let mut optional_seen = false;
        for key in keys.chars() {
            if key.is_ascii_lowercase() {
                optional_seen = true;
            } else if optional_seen {
                return Err(format!(
                    "required argument follows an optional argument in line: {line}"
                ));
            }
        }
        keys
    } else {
        String::new()
    };

    let availability = if external_suffix.is_some() {
        "external"
    } else {
        "internal-only"
    };
    let export_name = if external_suffix.is_some() {
        args[0].clone()
    } else {
        String::new()
    };

    Ok(Some(Entry {
        ordinal,
        macro_kind: base_kind.to_string(),
        availability: availability.to_string(),
        macro_name: args[0].clone(),
        export_name,
        target,
        min_args,
        flags,
        arg_keys,
        cme_name,
    }))
}

fn strip_toml_comment(line: &str) -> &str {
    let mut escaped = false;
    let mut in_string = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn toml_value(text: &str, section: &str, key: &str) -> Result<String, String> {
    let mut current_section = "";
    let mut found = None;
    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section_name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            current_section = section_name.trim();
            continue;
        }
        if current_section != section {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        if found.is_some() {
            return Err(format!("duplicate provenance key '{section}.{key}'"));
        }
        let value = value.trim();
        let parsed = if let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        {
            value.replace("\\\"", "\"")
        } else {
            value.to_string()
        };
        found = Some(parsed);
    }
    found.ok_or_else(|| format!("missing provenance key '{section}.{key}'"))
}

fn parse_provenance(path: &Path) -> Result<Provenance, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("reading provenance {}: {error}", path.display()))?;
    let parse_usize = |key: &str| {
        toml_value(&text, "expected", key)?
            .parse::<usize>()
            .map_err(|error| format!("provenance key '{key}' is not a valid usize: {error}"))
    };
    Ok(Provenance {
        pythonmodule_commit: toml_value(&text, "source", "pythonmodule_commit")?,
        serialem_commit: toml_value(&text, "source", "serialem_commit")?,
        expected_enum_entries: parse_usize("all_enum_entries")?,
        expected_external_wrappers: parse_usize("external_wrappers")?,
        file_path: toml_value(&text, "file", "path")?,
        repository_path: toml_value(&text, "file", "repository_path")?,
        git_blob_sha: toml_value(&text, "file", "git_blob_sha")?,
        sha256: toml_value(&text, "file", "sha256")?,
    })
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verify_provenance_file(path: &Path, provenance: &Provenance) -> Result<(), String> {
    let expected_name = Path::new(&provenance.file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "provenance file.path has no file name".to_string())?;
    let actual_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("input path {} has no file name", path.display()))?;
    if actual_name != expected_name {
        return Err(format!(
            "generator input {} does not match provenance file.path {}",
            path.display(),
            provenance.file_path
        ));
    }
    if !provenance.repository_path.ends_with(&provenance.file_path) {
        return Err(format!(
            "provenance repository_path {} does not end with file.path {}",
            provenance.repository_path, provenance.file_path
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "reading {} for provenance verification: {error}",
            path.display()
        )
    })?;
    let mut git_hasher = Sha1::new();
    git_hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    git_hasher.update(&bytes);
    let actual_git_blob = hex_digest(git_hasher.finalize());
    let actual_sha256 = hex_digest(Sha256::digest(&bytes));
    if !actual_git_blob.eq_ignore_ascii_case(&provenance.git_blob_sha) {
        return Err(format!(
            "Git blob SHA mismatch for {}: expected {}, got {}",
            path.display(),
            provenance.git_blob_sha,
            actual_git_blob
        ));
    }
    if !actual_sha256.eq_ignore_ascii_case(&provenance.sha256) {
        return Err(format!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            path.display(),
            provenance.sha256,
            actual_sha256
        ));
    }
    Ok(())
}

fn strip_cpp_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let character = bytes[index] as char;
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        match character {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '/' if !in_string && bytes.get(index + 1) == Some(&b'/') => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

fn logical_macro_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for raw_line in text.lines() {
        let line = strip_cpp_comment(raw_line).trim();
        if pending.is_empty() {
            if !line.starts_with("MAC_") {
                continue;
            }
        } else if line.is_empty() {
            continue;
        }
        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(line);
        for character in line.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            match character {
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '(' if !in_string => depth += 1,
                ')' if !in_string && depth > 0 => depth -= 1,
                _ => {}
            }
        }
        if depth == 0 && !pending.is_empty() {
            lines.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

fn parse_header(text: &str, provenance: &Provenance) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for line in logical_macro_lines(text) {
        if let Some(entry) = parse_macro_line(&line, entries.len())? {
            entries.push(entry);
        }
    }

    let external_count = entries
        .iter()
        .filter(|entry| entry.availability == "external")
        .count();
    if entries.len() != provenance.expected_enum_entries {
        return Err(format!(
            "expected {} enum entries, found {}",
            provenance.expected_enum_entries,
            entries.len()
        ));
    }
    if external_count != provenance.expected_external_wrappers {
        return Err(format!(
            "expected {} external wrappers, found {external_count}",
            provenance.expected_external_wrappers
        ));
    }
    Ok(entries)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[allow(non_snake_case)]
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING; generated destinations are not held open by this process.
    let result = unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), 0x1) };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid output path {}", path.display()))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&temporary, contents)
        .map_err(|error| format!("writing temporary {}: {error}", temporary.display()))?;
    if let Err(error) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("atomically replacing {}: {error}", path.display()));
    }
    Ok(())
}

fn validate_special_surface() -> Result<(), String> {
    let source_names: HashSet<&str> = SPECIALS.iter().map(|(name, _, _, _)| *name).collect();
    let mapping_names: HashSet<&str> = SPECIAL_MAPPINGS.iter().map(|(name, _, _)| *name).collect();
    if source_names.len() != SPECIALS.len() || mapping_names.len() != SPECIAL_MAPPINGS.len() {
        return Err("special API surface contains duplicate names".to_string());
    }
    if source_names != mapping_names {
        return Err("SPECIALS and SPECIAL_MAPPINGS names do not match".to_string());
    }
    Ok(())
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn write_csv(path: &Path, entries: &[Entry], provenance: &Provenance) -> Result<(), String> {
    let mut output = String::from(
        "ordinal,cme_id,macro_kind,availability,macro_name,export_name,target,min_args,flags,arg_keys,protocol,status,test_status,min_version,known_differences,notes\n",
    );
    for entry in entries {
        let protocol = if entry.availability == "external" {
            "PSS_RegularCommand"
        } else {
            ""
        };
        let notes = if entry.availability == "external" {
            "generated from MacroMasterList.h"
        } else {
            "enum-only entry; retained for CME numbering"
        };
        let status = if entry.availability == "external" {
            "generated"
        } else {
            "enum-only"
        };
        let test_status = if entry.availability == "external" {
            "compile-only"
        } else {
            "parser-count"
        };
        let known_differences = if entry.availability == "external" {
            "real SerialEM integration and per-command behavior pending"
        } else {
            "not externally callable"
        };
        let fields = [
            entry.ordinal.to_string(),
            format!("CME_{}", entry.cme_name),
            entry.macro_kind.clone(),
            entry.availability.clone(),
            entry.macro_name.clone(),
            entry.export_name.clone(),
            entry.target.clone(),
            entry.min_args.to_string(),
            entry.flags.to_string(),
            entry.arg_keys.clone(),
            protocol.to_string(),
            status.to_string(),
            test_status.to_string(),
            "pinned-source".to_string(),
            known_differences.to_string(),
            notes.to_string(),
        ];
        output.push_str(
            &fields
                .iter()
                .map(|field| csv_escape(field))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }

    for (index, (name, category, protocol, notes)) in SPECIALS.iter().enumerate() {
        let notes = if *name == "ReportNumModuleFuncs" {
            format!(
                "Reports the complete CME enum count ({} in the pinned baseline); external wrappers are tracked separately",
                provenance.expected_enum_entries
            )
        } else {
            (*notes).to_string()
        };
        let fields = [
            format!("special-{}", index + 1),
            String::new(),
            "SPECIAL".to_string(),
            "special".to_string(),
            (*name).to_string(),
            (*name).to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            (*protocol).to_string(),
            "implemented-core".to_string(),
            "unit/mock".to_string(),
            "pinned-source".to_string(),
            "real SerialEM integration remains pending".to_string(),
            format!("{category}: {notes}"),
        ];
        output.push_str(
            &fields
                .iter()
                .map(|field| csv_escape(field))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }

    atomic_write(path, &output)
}

fn write_markdown(path: &Path, entries: &[Entry], provenance: &Provenance) -> Result<(), String> {
    let external_count = entries
        .iter()
        .filter(|entry| entry.availability == "external")
        .count();
    let internal_count = entries.len() - external_count;
    let mut output = String::new();
    output.push_str("# SerialEM PythonModule API matrix\n\n");
    output.push_str(
        "This file is generated from the pinned `vendor/serialem/MacroMasterList.h`.\n\n",
    );
    output.push_str("## Baseline\n\n");
    output.push_str(&format!(
        "- PythonModule commit: `{}`\n",
        provenance.pythonmodule_commit
    ));
    output.push_str(&format!(
        "- SerialEM commit: `{}`\n",
        provenance.serialem_commit
    ));
    output.push_str(&format!(
        "- Complete CME enum entries: `{}`\n",
        entries.len()
    ));
    output.push_str(&format!(
        "- External generated wrappers: `{external_count}`\n"
    ));
    output.push_str(&format!(
        "- Enum-only/internal entries retained for numbering: `{internal_count}`\n\n"
    ));
    output.push_str(
        "The CSV contains one row for every enum entry plus the non-macro PythonModule surface.\n",
    );
    output.push_str("`min_args` is the SerialEM macro-command minimum; `arg_keys` describes the external wrapper's typed argument slots and may differ when compatibility/default handling is applied.\n");
    output.push_str("Command IDs must be assigned using every enum row, including rows that do not expose an external wrapper.\n\n");
    output.push_str("## Special PythonModule surface\n\n");
    output.push_str("| Name | Category | Protocol | Notes |\n|---|---|---|---|\n");
    for (name, category, protocol, notes) in SPECIALS {
        let notes = if *name == "ReportNumModuleFuncs" {
            format!(
                "Reports the complete CME enum count ({} in the pinned baseline); external wrappers are tracked separately",
                provenance.expected_enum_entries
            )
        } else {
            (*notes).to_string()
        };
        output.push_str(&format!(
            "| `{name}` | {category} | `{protocol}` | {notes} |\n"
        ));
    }
    output.push_str("\n## Rust mapping\n\n");
    output.push_str("| Python surface | Rust mapping | Status |\n|---|---|---|\n");
    for (name, mapping, status) in SPECIAL_MAPPINGS {
        let mapping = if *name == "ReportNumModuleFuncs" {
            format!(
                "SerialEmClient::report_num_module_funcs ({} upstream CME count)",
                provenance.expected_enum_entries
            )
        } else {
            (*mapping).to_string()
        };
        output.push_str(&format!("| `{name}` | `{mapping}` | {status} |\n"));
    }
    output.push_str("\n## Status\n\nExternal command rows are `generated`; enum-only rows preserve numbering; special surfaces are `implemented-core`. Real SerialEM integration and per-command golden coverage remain pending.\n");

    atomic_write(path, &output)
}

fn write_command_ids(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut output = String::from(
        "// @generated by generate_api.rs; do not edit.\n#![allow(non_upper_case_globals)]\n\n",
    );
    for entry in entries {
        writeln!(
            &mut output,
            "pub const CME_{}: i32 = {};",
            entry.cme_name, entry.ordinal
        )
        .map_err(|error| format!("formatting command ID output: {error}"))?;
    }
    writeln!(
        &mut output,
        "pub const CME_ENUM_LENGTH: usize = {};",
        entries.len()
    )
    .map_err(|error| format!("formatting command ID length: {error}"))?;
    output.push_str(
        "\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct CommandSpec {\n    pub cme_name: &'static str,\n    pub name: &'static str,\n    pub id: i32,\n    pub min_args: usize,\n    pub flags: i32,\n    pub arg_keys: &'static str,\n}\n\npub const EXTERNAL_COMMAND_SPECS: &[CommandSpec] = &[\n",
    );
    for entry in entries
        .iter()
        .filter(|entry| entry.availability == "external")
    {
        writeln!(
            &mut output,
            "    CommandSpec {{ cme_name: {:?}, name: {:?}, id: {}, min_args: {}, flags: {}, arg_keys: {:?} }},",
            format!("CME_{}", entry.cme_name),
            entry.export_name,
            entry.ordinal,
            entry.min_args,
            entry.flags,
            entry.arg_keys,
        )
        .map_err(|error| format!("formatting command metadata: {error}"))?;
    }
    output.push_str("];\n");
    atomic_write(path, &output)
}

fn rust_method_name(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do",
        "final", "gen", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield",
        "try", "union",
    ];
    if RESERVED.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

fn rust_type(key: char) -> Result<&'static str, String> {
    match key {
        'D' => Ok("f64"),
        'S' => Ok("&str"),
        'I' => Ok("i32"),
        'd' => Ok("Option<f64>"),
        's' => Ok("Option<&str>"),
        'i' => Ok("Option<i32>"),
        _ => Err(format!("unsupported argument key {key}")),
    }
}

fn helper_name(key: char) -> Result<&'static str, String> {
    match key.to_ascii_uppercase() {
        'D' => Ok("script_item_from_double"),
        'S' => Ok("script_item_from_string"),
        'I' => Ok("script_item_from_int"),
        _ => Err(format!("unsupported argument key {key}")),
    }
}

fn format_generated_rust(path: &Path) -> Result<(), String> {
    let status = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(path)
        .status()
        .map_err(|error| format!("running rustfmt for {}: {error}", path.display()))?;
    if !status.success() {
        return Err(format!(
            "rustfmt failed for {} with {status}",
            path.display()
        ));
    }
    Ok(())
}

fn write_command_wrappers(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut output = String::from(
        "// @generated by generate_api.rs; do not edit.\n#![allow(non_snake_case, unused_assignments, unused_mut, clippy::too_many_arguments)]\n\nuse super::command_ids::*;\nuse crate::client::{script_item_from_double, script_item_from_int, script_item_from_string, CommandResult, SerialEmClient};\nuse crate::error::Error;\nuse crate::protocol::ScriptItem;\n\n#[allow(non_snake_case)]\nimpl SerialEmClient {\n",
    );

    for entry in entries
        .iter()
        .filter(|entry| entry.availability == "external")
    {
        let method = rust_method_name(&entry.export_name);
        write!(&mut output, "    pub fn {method}(&mut self").map_err(|error| error.to_string())?;
        for (index, key) in entry.arg_keys.chars().enumerate() {
            let ty = rust_type(key)?;
            write!(&mut output, ", arg{}: {ty}", index + 1).map_err(|error| error.to_string())?;
        }
        output.push_str(") -> Result<CommandResult, Error> {\n");
        output.push_str("        let mut items = vec![ScriptItem::default()];\n");
        output.push_str("        let mut last_non_empty_index = 0usize;\n");
        if entry.arg_keys.chars().any(|key| key.is_ascii_lowercase()) {
            output.push_str("        let mut omitted_optional = false;\n");
        }

        for (index, key) in entry.arg_keys.chars().enumerate() {
            let arg_name = format!("arg{}", index + 1);
            let helper = helper_name(key)?;
            if key.is_ascii_lowercase() {
                writeln!(&mut output, "        if let Some(value) = {arg_name} {{")
                    .map_err(|error| error.to_string())?;
                output.push_str("            if omitted_optional {\n");
                output.push_str("                return Err(Error::InvalidArgument(\"an optional argument cannot be followed by another supplied argument\".to_string()));\n");
                output.push_str("            }\n");
                writeln!(&mut output, "            items.push({helper}(value));")
                    .map_err(|error| error.to_string())?;
                output.push_str("            last_non_empty_index += 1;\n");
                output.push_str("        } else {\n");
                output.push_str("            omitted_optional = true;\n");
                output.push_str("        }\n");
            } else {
                writeln!(&mut output, "        items.push({helper}({arg_name}));")
                    .map_err(|error| error.to_string())?;
                output.push_str("        last_non_empty_index += 1;\n");
            }
        }

        writeln!(
            &mut output,
            "        self.execute_generated(CME_{}, items, last_non_empty_index)\n    }}\n",
            entry.cme_name
        )
        .map_err(|error| error.to_string())?;
    }
    output.push_str("}\n");
    atomic_write(path, &output)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if !matches!(args.len(), 4 | 6 | 7) {
        return Err(format!(
            "usage: {} <MacroMasterList.h> <api-matrix.csv> <api-matrix.md> [command_ids.rs commands.rs [provenance.toml]]",
            args.first().map(String::as_str).unwrap_or("generate_api")
        ));
    }

    let header_path = Path::new(&args[1]);
    let csv_path = Path::new(&args[2]);
    let markdown_path = Path::new(&args[3]);
    let provenance_path = if args.len() == 7 {
        Path::new(&args[6]).to_path_buf()
    } else {
        header_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("provenance.toml")
    };
    let provenance = parse_provenance(&provenance_path)?;
    validate_special_surface()?;
    verify_provenance_file(header_path, &provenance)?;
    let header = fs::read_to_string(header_path)
        .map_err(|error| format!("reading {}: {error}", header_path.display()))?;
    let entries = parse_header(&header, &provenance)?;
    write_csv(csv_path, &entries, &provenance)?;
    write_markdown(markdown_path, &entries, &provenance)?;
    if args.len() >= 6 {
        let command_ids_path = Path::new(&args[4]);
        let commands_path = Path::new(&args[5]);
        write_command_ids(command_ids_path, &entries)?;
        write_command_wrappers(commands_path, &entries)?;
        format_generated_rust(command_ids_path)?;
        format_generated_rust(commands_path)?;
    }
    println!(
        "generated {} enum entries and {} external wrappers",
        entries.len(),
        entries
            .iter()
            .filter(|entry| entry.availability == "external")
            .count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provenance_values_without_hardcoded_counts() {
        let text = r#"
            [source]
            pythonmodule_commit = "python"
            serialem_commit = "serialem"
            [expected]
            all_enum_entries = 843
            external_wrappers = 793
        "#;
        assert_eq!(
            toml_value(text, "source", "pythonmodule_commit").unwrap(),
            "python"
        );
        assert_eq!(
            toml_value(text, "source", "serialem_commit").unwrap(),
            "serialem"
        );
        assert_eq!(
            toml_value(text, "expected", "all_enum_entries").unwrap(),
            "843"
        );
        assert_eq!(
            toml_value(text, "expected", "external_wrappers").unwrap(),
            "793"
        );
        assert!(toml_value(text, "source", "all_enum_entries").is_err());
    }

    #[test]
    fn rejects_duplicate_keys_and_preserves_hashes_inside_strings() {
        let text = r#"
            [source]
            pythonmodule_commit = "abc#123" # comment
            pythonmodule_commit = "duplicate"
        "#;
        assert_eq!(
            toml_value(text, "source", "pythonmodule_commit"),
            Err("duplicate provenance key 'source.pythonmodule_commit'".to_string())
        );
        let single = "[source]\npythonmodule_commit = \"abc#123\" # trailing";
        assert_eq!(
            toml_value(single, "source", "pythonmodule_commit").unwrap(),
            "abc#123"
        );
    }

    #[test]
    fn verifies_provenance_hashes_and_rejects_tampering() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let provenance_path = root.join("vendor/serialem/provenance.toml");
        let header_path = root.join("vendor/serialem/MacroMasterList.h");
        let provenance = parse_provenance(&provenance_path).unwrap();
        verify_provenance_file(&header_path, &provenance).unwrap();

        let mut tampered = provenance.clone();
        tampered.sha256 = "00".repeat(32);
        assert!(
            verify_provenance_file(&header_path, &tampered)
                .unwrap_err()
                .contains("SHA-256 mismatch")
        );

        let mut wrong_path = provenance;
        wrong_path.file_path = "OtherHeader.h".to_string();
        assert!(
            verify_provenance_file(&header_path, &wrong_path)
                .unwrap_err()
                .contains("does not match provenance file.path")
        );
    }

    #[test]
    fn parses_multiline_macro_lines_and_strict_special_surface() {
        let text = r#"MAC_SAME_NAME_ARG(
  TiltUp, 0, 0, TILTUP, idd // comment
)"#;
        let lines = logical_macro_lines(text);
        assert_eq!(lines.len(), 1);
        let entry = parse_macro_line(&lines[0], 0).unwrap().unwrap();
        assert_eq!(entry.cme_name, "TILTUP");
        validate_special_surface().unwrap();
    }

    #[test]
    fn parses_required_and_optional_argument_keys() {
        let entry = parse_macro_line("MAC_SAME_NAME_ARG(SetVariable, 0, 4, SETVARIABLE, SS)", 2)
            .unwrap()
            .unwrap();
        assert_eq!(entry.export_name, "SetVariable");
        assert_eq!(entry.min_args, 0);
        assert_eq!(entry.flags, 4);
        assert_eq!(entry.arg_keys, "SS");
        assert_eq!(entry.ordinal, 2);

        let optional = parse_macro_line("MAC_SAME_NAME_ARG(TiltUp, 0, 0, TILTUP, idd)", 3)
            .unwrap()
            .unwrap();
        assert_eq!(optional.arg_keys, "idd");
    }

    #[test]
    fn rejects_required_arguments_after_optional_keys() {
        let error = parse_macro_line("MAC_SAME_NAME_ARG(Bad, 0, 0, BAD, dI)", 0).unwrap_err();
        assert!(error.contains("required argument follows"));
    }

    #[test]
    fn rejects_unknown_argument_keys() {
        let error = parse_macro_line("MAC_SAME_NAME_ARG(Bad, 0, 0, BAD, X)", 0).unwrap_err();
        assert!(error.contains("invalid argument keys"));
    }
}
