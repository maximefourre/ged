//! Download-facing file names: the `Content-Disposition` header Rails'
//! `send_file` builds, and admin ZIP entry names. Both take a stored file name
//! that may be hostile (quotes, CR/LF, Unicode, `../`).

use super::transliteration_table::APPROXIMATIONS;

/// `ActionDispatch::Http::ContentDisposition.format`:
/// `attachment; filename="<ascii fallback>"; filename*=UTF-8''<rfc 5987>`.
/// Every byte outside a tiny safe set is percent-escaped, so a quote, CR/LF or
/// `;` in a name cannot end the parameter or the header.
pub fn content_disposition(inline: bool, filename: &str) -> String {
    let kind = if inline { "inline" } else { "attachment" };
    let ascii = percent_escape(&transliterate(filename), |b| {
        b.is_ascii_alphanumeric() || b" !#$+.^_`|~-".contains(&b)
    });
    let utf8 = percent_escape(filename, |b| {
        b.is_ascii_alphanumeric() || b"!#$&+.^_`|~-".contains(&b)
    });
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{utf8}")
}

/// Admin bulk-download entry name: `"#{id}-#{filename}"`. Rails keeps the name
/// as stored; a stored name that is only a path separator (`/`) would turn the
/// entry into a directory, so separators become `_` (see parity-gaps.md).
pub fn zip_entry_name(id: &str, filename: &str) -> String {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{id}-{safe}")
}

/// `I18n.transliterate`: non-ASCII characters become their approximation, or `?`.
fn transliterate(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let key: &str = c.encode_utf8(&mut buf);
        match APPROXIMATIONS.binary_search_by_key(&key, |(k, _)| k) {
            Ok(i) => out.push_str(APPROXIMATIONS[i].1),
            Err(_) => out.push('?'),
        }
    }
    out
}

fn percent_escape(text: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if keep(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Corpus {
        id: String,
        cases: Vec<Case>,
    }

    #[derive(serde::Deserialize)]
    struct Case {
        input: String,
        normalized: String,
        attachment: String,
        inline: String,
        zip_entry: String,
    }

    fn corpus() -> Corpus {
        serde_json::from_str(include_str!("../../tests/fixtures/filename_corpus.json")).unwrap()
    }

    /// Header and entry names built from what Rails stores, byte for byte.
    #[test]
    fn headers_and_zip_entries_match_rails_for_hostile_names() {
        let corpus = corpus();
        let mut failures = Vec::new();
        for case in &corpus.cases {
            let name = &case.normalized;
            for (label, got, want) in [
                (
                    "attachment",
                    content_disposition(false, name),
                    &case.attachment,
                ),
                ("inline", content_disposition(true, name), &case.inline),
                ("zip", zip_entry_name(&corpus.id, name), &case.zip_entry),
            ] {
                // the one deliberate difference: separators in a zip entry
                if label == "zip" && name.contains('/') {
                    assert_eq!(got, format!("{}-{}", corpus.id, name.replace('/', "_")));
                    continue;
                }
                if &got != want {
                    failures.push(format!(
                        "{:?} {label}: rails={want:?} rust={got:?}",
                        case.input
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The header can never be split or smuggle a parameter, whatever the name.
    #[test]
    fn disposition_is_one_header_line_of_printable_ascii() {
        for case in &corpus().cases {
            for inline in [false, true] {
                let header = content_disposition(inline, &case.normalized);
                assert!(
                    header.bytes().all(|b| (0x20..0x7f).contains(&b)),
                    "{:?} -> {header:?}",
                    case.input
                );
                // exactly: <disposition>; filename="..."; filename*=UTF-8''...
                let quoted = header.split("filename=\"").nth(1).unwrap();
                let value = quoted.split('"').next().unwrap();
                assert!(!value.contains(';') && !value.contains('\\'), "{header:?}");
                assert_eq!(
                    header.matches('"').count(),
                    2,
                    "{:?} -> {header:?}",
                    case.input
                );
                assert_eq!(header.matches("; filename").count(), 2, "{header:?}");
            }
        }
    }

    /// An entry never leaves its directory nor carries control characters.
    #[test]
    fn zip_entries_stay_flat_and_clean() {
        for case in &corpus().cases {
            let entry = zip_entry_name(&corpus().id, &case.normalized);
            assert!(!entry.contains('/') && !entry.contains('\\'), "{entry:?}");
            assert!(!entry.chars().any(|c| c.is_control()), "{entry:?}");
            assert!(!entry.starts_with('.'), "{entry:?}");
        }
    }
}
