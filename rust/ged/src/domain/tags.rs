//! `KvTaggable`: a single-level key→value tag map in a jsonb column.
//! Keys are stripped and downcased; values are stripped strings (blank becomes
//! null) or null. Anything malformed passes through normalisation untouched so
//! validation rejects it instead of masking it.

use serde_json::{Map, Value};

pub const MAX_ENTRIES: usize = 50;
pub const MAX_KEY_LENGTH: usize = 100;
pub const MAX_VALUE_LENGTH: usize = 500;

/// Ruby's `String#strip`: ASCII whitespace and NUL only.
fn ruby_strip(text: &str) -> &str {
    text.trim_matches(|c| matches!(c, '\0' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' '))
}

/// Ruby's `String#downcase` maps character by character (no final-sigma rule).
fn ruby_downcase(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// `KvTaggable#normalize_tags`.
pub fn normalize(value: Value) -> Value {
    let Value::Object(map) = value else {
        return value;
    };
    let mut out = Map::with_capacity(map.len());
    for (key, val) in map {
        let key = ruby_downcase(ruby_strip(&key));
        let val = match val {
            // `strip.presence`: blank (Unicode spaces too) collapses to null
            Value::String(text) => {
                let stripped = ruby_strip(&text);
                if stripped.chars().all(char::is_whitespace) {
                    Value::Null
                } else {
                    Value::String(stripped.to_owned())
                }
            }
            other => other,
        };
        out.insert(key, val);
    }
    Value::Object(out)
}

/// `tags_must_be_a_valid_map`: the messages Rails adds to `errors[:tags]`,
/// in order.
pub fn validate(tags: &Value) -> Vec<String> {
    let Value::Object(map) = tags else {
        return vec!["must be a map of key/value pairs".to_owned()];
    };
    let mut errors = Vec::new();
    if map.len() > MAX_ENTRIES {
        errors.push(format!("cannot have more than {MAX_ENTRIES} entries"));
    }
    let key_ok =
        |k: &String| !k.chars().all(char::is_whitespace) && k.chars().count() <= MAX_KEY_LENGTH;
    if !map.keys().all(key_ok) {
        errors.push(format!(
            "keys must be non-empty strings of at most {MAX_KEY_LENGTH} characters"
        ));
    }
    let value_ok = |v: &Value| match v {
        Value::Null => true,
        Value::String(text) => text.chars().count() <= MAX_VALUE_LENGTH,
        _ => false,
    };
    if !map.values().all(value_ok) {
        errors.push(format!(
            "values must be strings of at most {MAX_VALUE_LENGTH} characters, or null"
        ));
    }
    errors
}

/// `errors.full_messages` entries for the tags attribute.
pub fn full_messages(errors: &[String]) -> Vec<String> {
    errors.iter().map(|e| format!("Tags {e}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_stripped_and_downcased_values_stripped() {
        assert_eq!(
            normalize(json!({" Type ": "  facture ", "URGENT": null})),
            json!({"type": "facture", "urgent": null})
        );
    }

    #[test]
    fn blank_values_collapse_to_null_including_unicode_spaces() {
        assert_eq!(
            normalize(json!({"a": "   ", "b": "", "c": "\u{a0}", "d": "\u{3000}\t"})),
            json!({"a": null, "b": null, "c": null, "d": null})
        );
    }

    #[test]
    fn strip_is_ascii_whitespace_and_nul_only_like_ruby() {
        assert_eq!(normalize(json!({"a": "\u{0} x\u{0}\n"})), json!({"a": "x"}));
        assert_eq!(
            normalize(json!({"a": "\u{a0}x\u{a0}"})),
            json!({"a": "\u{a0}x\u{a0}"})
        );
    }

    #[test]
    fn downcase_is_per_character_like_ruby_without_final_sigma() {
        assert_eq!(normalize(json!({"ΑΣ": "v"})), json!({"ασ": "v"}));
        assert_eq!(normalize(json!({"İ": "v"})), json!({"i\u{307}": "v"}));
    }

    #[test]
    fn colliding_keys_keep_first_position_last_value() {
        let out = normalize(json!({"a": "1", "b": "2", "A": "3"}));
        assert_eq!(out.to_string(), r#"{"a":"3","b":"2"}"#);
    }

    #[test]
    fn non_string_values_and_non_maps_pass_through() {
        assert_eq!(
            normalize(json!({"a": 1, "b": ["x"]})),
            json!({"a": 1, "b": ["x"]})
        );
        assert_eq!(normalize(json!(["a"])), json!(["a"]));
        assert_eq!(normalize(json!("x")), json!("x"));
        assert_eq!(normalize(Value::Null), Value::Null);
    }

    #[test]
    fn valid_maps_have_no_errors() {
        assert!(validate(&json!({})).is_empty());
        assert!(validate(&json!({"type": "facture", "urgent": null})).is_empty());
    }

    #[test]
    fn a_non_map_has_only_the_map_error() {
        for bad in [json!(["a"]), json!("x"), json!(3), Value::Null] {
            assert_eq!(validate(&bad), vec!["must be a map of key/value pairs"]);
        }
    }

    #[test]
    fn entry_count_limit() {
        let ok: Map<String, Value> = (0..50).map(|i| (format!("k{i}"), Value::Null)).collect();
        assert!(validate(&Value::Object(ok.clone())).is_empty());
        let mut too_many = ok;
        too_many.insert("k50".into(), Value::Null);
        assert_eq!(
            validate(&Value::Object(too_many)),
            vec!["cannot have more than 50 entries"]
        );
    }

    #[test]
    fn key_rules_count_characters_not_bytes() {
        let key_ok = "é".repeat(100);
        assert!(validate(&json!({ key_ok: "v" })).is_empty());
        let key_long = "é".repeat(101);
        assert_eq!(
            validate(&json!({ key_long: "v" })),
            vec!["keys must be non-empty strings of at most 100 characters"]
        );
        assert_eq!(
            validate(&json!({"": "v"})),
            vec!["keys must be non-empty strings of at most 100 characters"]
        );
        assert_eq!(
            validate(&json!({"  ": "v"})),
            vec!["keys must be non-empty strings of at most 100 characters"]
        );
    }

    #[test]
    fn value_rules() {
        assert!(validate(&json!({"a": "é".repeat(500)})).is_empty());
        let msg = "values must be strings of at most 500 characters, or null";
        assert_eq!(validate(&json!({"a": "é".repeat(501)})), vec![msg]);
        assert_eq!(validate(&json!({"a": 1})), vec![msg]);
        assert_eq!(validate(&json!({"a": ["x"]})), vec![msg]);
        assert_eq!(validate(&json!({"a": {"b": "c"}})), vec![msg]);
    }

    #[test]
    fn errors_come_in_rails_order_and_full_messages_prefix_the_attribute() {
        let mut entries: Map<String, Value> =
            (0..50).map(|i| (format!("k{i}"), Value::Null)).collect();
        entries.insert(String::new(), json!(1));
        let errors = validate(&Value::Object(entries));
        assert_eq!(
            full_messages(&errors),
            vec![
                "Tags cannot have more than 50 entries",
                "Tags keys must be non-empty strings of at most 100 characters",
                "Tags values must be strings of at most 500 characters, or null",
            ]
        );
    }
}
