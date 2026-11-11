//! The parity inventory's Cible column is the coverage map. A path that does
//! not exist is a silent gap, which T16 left in place for eight rows.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `(line number, repo-relative path)` from numbered inventory rows.
fn inventory_targets(doc: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (idx, line) in doc.lines().enumerate() {
        let line_no = idx + 1;
        if !line.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cols.len() < 5 || cols[0].is_empty() || !cols[0].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let mut rest = cols[4];
        let mut row_paths = Vec::new();
        while let Some(start) = rest.find('`') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('`') else { break };
            let path = &rest[..end];
            if let Some(relative) = path.strip_prefix("rust/")
                && !relative.is_empty()
            {
                row_paths.push(path.to_owned());
            }
            rest = &rest[end + 1..];
        }
        assert!(
            !row_paths.is_empty(),
            "inventory line {line_no} has no rust/ path in Cible: {}",
            cols[4]
        );
        for path in row_paths {
            found.push((line_no, path));
        }
    }
    assert!(
        !found.is_empty(),
        "the inventory table was not parsed; the Cible column would go unchecked"
    );
    found
}

fn missing_targets(doc: &str, root: &Path) -> Vec<String> {
    inventory_targets(doc)
        .into_iter()
        .filter(|(_, path)| !root.join(path).is_file())
        .map(|(line, path)| format!("line {line}: {path}"))
        .collect()
}

#[test]
fn every_inventory_target_names_a_file_that_exists() {
    let doc = fs::read_to_string(root().join("docs/migration/parity-inventory.md"))
        .expect("read parity-inventory.md");
    let missing = missing_targets(&doc, &root());
    assert!(
        missing.is_empty(),
        "parity inventory targets that are not files:\n{}",
        missing.join("\n")
    );
}

#[test]
fn a_target_that_is_not_a_file_fails_the_check() {
    let doc = "| 9 | `test/x.rb:1` | name | parité | `rust/missing.rs` | |\n";
    let missing = missing_targets(doc, &std::env::temp_dir());
    assert_eq!(missing, vec!["line 1: rust/missing.rs".to_owned()]);
}
