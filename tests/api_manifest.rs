use std::collections::HashSet;

use serialem_client::generated::command_ids::{CME_ENUM_LENGTH, EXTERNAL_COMMAND_SPECS};

const API_MATRIX: &str = include_str!("../docs/api-matrix.csv");
const GENERATED_COMMANDS: &str = include_str!("../src/generated/commands.rs");
const PROVENANCE: &str = include_str!("../vendor/serialem/provenance.toml");

fn provenance_usize(key: &str) -> usize {
    let mut section = "";
    PROVENANCE
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                section = name;
                return None;
            }
            let (name, value) = line.split_once('=')?;
            (section == "expected" && name.trim() == key)
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap()
}

fn parse_csv_record(line: &str) -> Result<Vec<String>, String> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut field));
            }
            _ => field.push(character),
        }
    }
    if quoted {
        return Err("unterminated quoted CSV field".to_string());
    }
    fields.push(field);
    Ok(fields)
}

#[test]
fn generated_surface_matches_the_pinned_api_matrix() {
    let mut external_names: HashSet<String> = HashSet::new();
    let mut external_rows = Vec::new();
    let mut enum_only_count = 0usize;
    let mut special_names = HashSet::new();
    for (line_number, line) in API_MATRIX.lines().enumerate().skip(1) {
        let fields = parse_csv_record(line)
            .unwrap_or_else(|error| panic!("matrix row {line_number} is invalid CSV: {error}"));
        assert!(
            fields.len() >= 16,
            "matrix row {line_number} has too few fields"
        );
        match fields[3].as_str() {
            "external" => {
                assert!(
                    external_names.insert(fields[5].clone()),
                    "duplicate wrapper {}",
                    fields[5]
                );
                assert_eq!(fields[11], "generated");
                external_rows.push(fields);
            }
            "internal-only" => {
                enum_only_count += 1;
                assert_eq!(fields[14], "not externally callable");
            }
            "special" => {
                special_names.insert(fields[4].clone());
                assert_eq!(fields[11], "implemented-core");
            }
            availability => panic!("unexpected matrix availability {availability}"),
        }
    }

    let generated_names: HashSet<String> = GENERATED_COMMANDS
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub fn "))
        .filter_map(|signature| signature.split('(').next())
        .map(str::to_string)
        .collect();

    let expected_enum_entries = provenance_usize("all_enum_entries");
    let expected_external_wrappers = provenance_usize("external_wrappers");
    assert_eq!(CME_ENUM_LENGTH, expected_enum_entries);
    assert_eq!(external_names.len(), expected_external_wrappers);
    assert_eq!(external_rows.len(), expected_external_wrappers);
    assert_eq!(generated_names.len(), expected_external_wrappers);
    assert_eq!(EXTERNAL_COMMAND_SPECS.len(), expected_external_wrappers);
    assert_eq!(
        enum_only_count,
        expected_enum_entries - expected_external_wrappers
    );
    assert_eq!(special_names.len(), 10);
    assert_eq!(external_names, generated_names);

    for spec in EXTERNAL_COMMAND_SPECS {
        let row = external_rows
            .iter()
            .find(|fields| fields[5] == spec.name)
            .unwrap_or_else(|| panic!("missing manifest row for {}", spec.name));
        assert_eq!(row[1], spec.cme_name, "CME name mismatch for {}", spec.name);
        assert_eq!(
            row[0].parse::<i32>().unwrap(),
            spec.id,
            "CME id mismatch for {}",
            spec.name
        );
        assert_eq!(
            row[7].parse::<usize>().unwrap(),
            spec.min_args,
            "minimum args mismatch for {}",
            spec.name
        );
        assert_eq!(
            row[8].parse::<i32>().unwrap(),
            spec.flags,
            "flags mismatch for {}",
            spec.name
        );
        assert_eq!(
            row[9], spec.arg_keys,
            "argument keys mismatch for {}",
            spec.name
        );
    }
}

#[test]
fn csv_parser_preserves_quoted_commas_and_quotes() {
    let fields = parse_csv_record(r#"a,"quoted, value",c"#).unwrap();
    assert_eq!(fields, vec!["a", "quoted, value", "c"]);
}
