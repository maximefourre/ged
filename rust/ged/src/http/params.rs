//! Request parameters, parsed the way Rails 8.1 does it.
//!
//! This is a port of `ActionDispatch::ParamBuilder` (nested `a[b][]=…` keys,
//! last duplicate wins, `a[]` collects, type conflicts are a 400) plus the
//! parts of `ActionDispatch::Request` that decide what the handlers see:
//! nils are dropped from arrays ("deep munge"), strings must be valid UTF-8,
//! and a JSON body that is not an object is exposed as `_json`.
//!
//! Handlers read parameters with [`RackParams::str`], which returns a value
//! only for a plain string: `limit[]=1` and `limit[a]=1` are *present but not
//! scalar*, and so are ignored by the pagination code exactly as in Rails
//! (`value.is_a?(String) ? value.to_i : 0`).

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Method, request::Parts};
use http_body_util::BodyExt;

use super::error::ApiError;

/// `ActionDispatch::ParamBuilder.default.param_depth_limit`.
pub const PARAM_DEPTH_LIMIT: usize = 100;
/// ActiveSupport's `JSON.parse` `max_nesting`.
pub const JSON_MAX_NESTING: usize = 100;
/// Largest form or JSON body the extractor reads into memory. Rack caps
/// urlencoded input lower (4 MiB, 4096 keys; see `docs/migration/parity-gaps.md`),
/// so this is the more permissive side; uploads are multipart and stream elsewhere.
pub const BODY_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamsError {
    /// `ActionDispatch::ParamError`: type conflict, too deep, bad encoding.
    Invalid(String),
    /// `ActionDispatch::Http::Parameters::ParseError`: unreadable JSON body.
    MalformedJson,
}

impl From<ParamsError> for ApiError {
    fn from(_: ParamsError) -> Self {
        // Rails renders both as the generic, status-derived 400.
        ApiError::bad_request()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// `a` without `=` (and JSON `null`).
    Nil,
    Str(String),
    /// JSON number, kept as sent: ids legitimately arrive as integers.
    Num(serde_json::Number),
    Bool(bool),
    Array(Vec<ParamValue>),
    Map(ParamMap),
}

impl ParamValue {
    /// The string, if this is a plain string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&ParamMap> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[ParamValue]> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    fn is_blank_slot(&self) -> bool {
        matches!(self, Self::Nil)
    }
}

/// An insertion-ordered string-keyed map (a Ruby Hash).
///
/// `entries` holds the order; `index` maps a key to its slot so that `get`
/// and `insert` are amortised O(1). A request can carry hundreds of
/// thousands of distinct keys before authentication, so a linear scan per
/// insert (quadratic overall) is a CPU denial of service. Keys are never
/// removed, so slots are stable.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParamMap {
    entries: Vec<(String, ParamValue)>,
    index: HashMap<String, usize>,
}

impl ParamMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&ParamValue> {
        self.index.get(key).map(|&slot| &self.entries[slot].1)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut ParamValue> {
        let slot = *self.index.get(key)?;
        Some(&mut self.entries[slot].1)
    }

    /// Ruby `Hash#[]=`: an existing key keeps its position.
    pub fn insert(&mut self, key: String, value: ParamValue) {
        match self.index.get(&key) {
            Some(&slot) => self.entries[slot].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Consumes the map, yielding entries in insertion order.
    fn into_entries(self) -> Vec<(String, ParamValue)> {
        self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// What `store_nested_param` returns in Ruby: the hash it was given, an array
/// (for `[]` with no container), or nil (empty key).
enum Stored {
    Nil,
    Hash,
    Array(Vec<ParamValue>),
}

/// The merged parameters of one request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RackParams {
    map: ParamMap,
}

impl RackParams {
    /// Parses a query string or an urlencoded form body.
    pub fn parse(query: &str) -> Result<Self, ParamsError> {
        let mut map = ParamMap::new();
        for (name, value) in each_pair(query)? {
            store_pair(&mut map, name, value)?;
        }
        Ok(Self { map })
    }

    /// Parses a JSON request body the way Rails' JSON params parser does:
    /// an object is the parameter hash, anything else becomes `{"_json": …}`.
    pub fn parse_json(body: &[u8]) -> Result<Self, ParamsError> {
        let cleaned = strip_json_comments(body).ok_or(ParamsError::MalformedJson)?;
        let value: serde_json::Value =
            serde_json::from_slice(&cleaned).map_err(|_| ParamsError::MalformedJson)?;
        if json_depth(&value) > JSON_MAX_NESTING {
            return Err(ParamsError::MalformedJson);
        }
        let mut map = ParamMap::new();
        match value {
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    map.insert(key, from_json(value));
                }
            }
            other => map.insert("_json".into(), from_json(other)),
        }
        Ok(Self { map })
    }

    /// Request body parameters overlaid with the query: on a clash the query
    /// wins, as in `request_parameters.merge(query_parameters)`.
    pub fn merged(body: RackParams, query: RackParams) -> Self {
        let mut map = body.map;
        for (key, value) in query.map.into_entries() {
            map.insert(key, value);
        }
        Self { map }
    }

    pub fn get(&self, key: &str) -> Option<&ParamValue> {
        self.map.get(key)
    }

    /// The parameter as a string. `None` when it is absent, `nil`, or not a
    /// plain string (array, hash, number, …).
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ParamValue::as_str)
    }

    pub fn map(&self) -> &ParamMap {
        &self.map
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Parses the query string of a request (eagerly validated by the router
/// guard so that every matched route answers 400 like Rails does).
impl<S: Send + Sync> FromRequestParts<S> for RackParams {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::parse(parts.uri.query().unwrap_or(""))?)
    }
}

/// Query **and** body: urlencoded forms and JSON are read (bounded by
/// [`BODY_LIMIT`]); multipart bodies are not parsed here.
pub struct RequestParams(pub RackParams);

impl<S: Send + Sync> FromRequest<S> for RequestParams {
    type Rejection = ApiError;

    async fn from_request(req: Request, _: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let query = RackParams::parse(parts.uri.query().unwrap_or(""))?;
        let kind = BodyKind::of(&parts.method, parts.headers.get(CONTENT_TYPE));
        if matches!(kind, BodyKind::Other) {
            return Ok(Self(query));
        }
        let bytes = read_body(body).await?;
        if bytes.is_empty() {
            return Ok(Self(query));
        }
        let body_params = match kind {
            BodyKind::Json => RackParams::parse_json(&bytes)?,
            BodyKind::Form => RackParams::parse(
                std::str::from_utf8(&bytes).map_err(|_| ApiError::bad_request())?,
            )?,
            BodyKind::Other => return Ok(Self(query)),
        };
        Ok(Self(RackParams::merged(body_params, query)))
    }
}

async fn read_body(body: Body) -> Result<bytes::Bytes, ApiError> {
    let limited = http_body_util::Limited::new(body, BODY_LIMIT);
    limited
        .collect()
        .await
        .map(|c| c.to_bytes())
        .map_err(|_| ApiError::bad_request())
}

#[derive(Debug, PartialEq, Eq)]
pub enum BodyKind {
    Json,
    Form,
    Other,
}

impl BodyKind {
    /// Rails' `content_mime_type` (`^([^,;]*)`, trimmed, lower-cased) against
    /// the registered JSON synonyms; Rack's `form_data?` for forms.
    pub fn of(method: &Method, content_type: Option<&axum::http::HeaderValue>) -> Self {
        let Some(value) = content_type.and_then(|v| v.to_str().ok()) else {
            // POST without a content type is form data to Rack.
            return if method == Method::POST {
                Self::Form
            } else {
                Self::Other
            };
        };
        let media = value
            .split([',', ';'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match media.as_str() {
            "application/json" | "text/x-json" | "application/jsonrequest" => Self::Json,
            "application/x-www-form-urlencoded" => Self::Form,
            _ => Self::Other,
        }
    }
}

// --- parsing -------------------------------------------------------------

/// `ActionDispatch::QueryParser.each_pair`: split on `&` (swallowing spaces
/// that follow it), skip empty parts, split once on `=`, URL-decode both.
/// A part without `=` has a `nil` value.
type RawPair = (Vec<u8>, Option<Vec<u8>>);

fn each_pair(query: &str) -> Result<Vec<RawPair>, ParamsError> {
    let mut pairs = Vec::new();
    let mut rest = query;
    while !rest.is_empty() {
        let (part, tail) = match rest.find('&') {
            Some(i) => (&rest[..i], rest[i + 1..].trim_start_matches(' ')),
            None => (rest, ""),
        };
        rest = tail;
        if part.is_empty() {
            continue;
        }
        let (name, value) = match part.split_once('=') {
            Some((n, v)) => (decode(n)?, Some(decode(v)?)),
            None => (decode(part)?, None),
        };
        pairs.push((name, value));
    }
    Ok(pairs)
}

/// `URI.decode_www_form_component`: `+` is a space, `%HH` is a byte, and a
/// `%` that is not followed by two hex digits is an `ArgumentError` (a 400).
fn decode(input: &str) -> Result<Vec<u8>, ParamsError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let digits = bytes
                    .get(i + 1)
                    .copied()
                    .and_then(hex)
                    .zip(bytes.get(i + 2).copied().and_then(hex));
                let Some((hi, lo)) = digits else {
                    return Err(ParamsError::Invalid(format!(
                        "invalid %-encoding ({input})"
                    )));
                };
                out.push(hi << 4 | lo);
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    Ok(out)
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn invalid_encoding(what: &str) -> ParamsError {
    ParamsError::Invalid(format!("Invalid encoding for parameter: {what}"))
}

/// Depth-0 handling of one pair: split off the top-level key, validate the
/// encodings Rails validates, then descend.
fn store_pair(
    map: &mut ParamMap,
    name: Vec<u8>,
    value: Option<Vec<u8>>,
) -> Result<(), ParamsError> {
    let name = String::from_utf8(name)
        .map_err(|e| invalid_encoding(&String::from_utf8_lossy(e.as_bytes())))?;
    let value = match value {
        None => None,
        Some(bytes) => {
            // Ruby checks the value only once the key is known to be
            // non-empty (`return if k.empty?` comes first).
            let top_key_empty = top_key(&name).is_empty();
            match String::from_utf8(bytes) {
                Ok(s) => Some(s),
                Err(_) if top_key_empty => return Ok(()),
                Err(e) => return Err(invalid_encoding(&String::from_utf8_lossy(e.as_bytes()))),
            }
        }
    };
    store_nested(map, &name, value, 0)?;
    Ok(())
}

/// Ruby `name.index("[", 1)`: the first `[` after the first character.
fn bracket_after_first(name: &str) -> Option<usize> {
    let first = name.chars().next().map_or(0, char::len_utf8);
    name[first..].find('[').map(|i| i + first)
}

fn top_key(name: &str) -> &str {
    match bracket_after_first(name) {
        Some(i) => &name[..i],
        None => name,
    }
}

/// `ParamBuilder#store_nested_param`.
fn store_nested(
    params: &mut ParamMap,
    name: &str,
    v: Option<String>,
    depth: usize,
) -> Result<Stored, ParamsError> {
    if depth >= PARAM_DEPTH_LIMIT {
        return Err(ParamsError::Invalid(
            "exceeded available parameter key space".into(),
        ));
    }

    let (k, after): (&str, &str) = if depth == 0 {
        match bracket_after_first(name) {
            Some(i) => (&name[..i], &name[i..]),
            None => (name, ""),
        }
    } else if let Some(rest) = name.strip_prefix("[]") {
        ("[]", rest)
    } else if name.starts_with('[') && name[1..].contains(']') {
        let close = 1 + name[1..].find(']').unwrap_or(0);
        (&name[1..close], &name[close + 1..])
    } else {
        (name, "")
    };

    if k.is_empty() {
        return Ok(Stored::Nil);
    }

    let value = match v {
        Some(s) => ParamValue::Str(s),
        None => ParamValue::Nil,
    };

    if after.is_empty() {
        if k == "[]" && depth != 0 {
            // deep munge: a nil never lands in an array.
            return Ok(Stored::Array(match value {
                ParamValue::Nil => vec![],
                v => vec![v],
            }));
        }
        params.insert(k.to_owned(), value);
    } else if after == "[" {
        params.insert(name.to_owned(), value);
    } else if after == "[]" {
        let array = array_slot(params, k)?;
        if !value.is_blank_slot() {
            array.push(value);
        }
    } else if let Some(rest) = after.strip_prefix("[]") {
        // `x[][y]`: a hash inside an array.
        let child_key = match rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            Some(inner) if !inner.is_empty() && !inner.contains('[') && !inner.contains(']') => {
                inner
            }
            _ => rest,
        };
        let array = array_slot(params, k)?;
        let reuse_last = matches!(
            array.last(),
            Some(ParamValue::Map(last)) if !map_has_key_path(last, child_key)
        );
        if reuse_last {
            if let Some(ParamValue::Map(last)) = array.last_mut() {
                store_nested(last, child_key, value_to_opt(value), depth + 1)?;
            }
        } else {
            let mut fresh = ParamMap::new();
            let stored = store_nested(&mut fresh, child_key, value_to_opt(value), depth + 1)?;
            array.push(match stored {
                Stored::Nil => ParamValue::Nil,
                Stored::Hash => ParamValue::Map(fresh),
                Stored::Array(a) => ParamValue::Array(a),
            });
        }
    } else {
        if params.get(k).is_none_or(ParamValue::is_blank_slot) {
            params.insert(k.to_owned(), ParamValue::Map(ParamMap::new()));
        }
        let Some(slot) = params.get_mut(k) else {
            return Ok(Stored::Hash);
        };
        let ParamValue::Map(inner) = slot else {
            return Err(ParamsError::Invalid(format!(
                "expected Hash (got {}) for param `{k}'",
                type_name(slot)
            )));
        };
        let mut taken = std::mem::take(inner);
        let stored = store_nested(&mut taken, after, value_to_opt(value), depth + 1);
        // Put the (possibly partially filled) map back before propagating.
        *slot = match &stored {
            Ok(Stored::Nil) => ParamValue::Nil,
            Ok(Stored::Array(a)) => ParamValue::Array(a.clone()),
            _ => ParamValue::Map(taken),
        };
        stored?;
    }
    Ok(Stored::Hash)
}

fn value_to_opt(value: ParamValue) -> Option<String> {
    match value {
        ParamValue::Str(s) => Some(s),
        _ => None,
    }
}

/// `params[k] ||= []` then the Array type check.
fn array_slot<'a>(
    params: &'a mut ParamMap,
    k: &str,
) -> Result<&'a mut Vec<ParamValue>, ParamsError> {
    if params.get(k).is_none_or(ParamValue::is_blank_slot) {
        params.insert(k.to_owned(), ParamValue::Array(vec![]));
    }
    match params.get_mut(k) {
        Some(ParamValue::Array(a)) => Ok(a),
        Some(other) => Err(ParamsError::Invalid(format!(
            "expected Array (got {}) for param `{k}'",
            type_name(other)
        ))),
        None => Err(ParamsError::Invalid(format!("missing param `{k}'"))),
    }
}

fn type_name(v: &ParamValue) -> &'static str {
    match v {
        ParamValue::Nil => "NilClass",
        ParamValue::Str(_) => "String",
        ParamValue::Num(_) => "Integer",
        ParamValue::Bool(_) => "TrueClass",
        ParamValue::Array(_) => "Array",
        ParamValue::Map(_) => "ActiveSupport::HashWithIndifferentAccess",
    }
}

/// `params_hash_has_key?`: does `hash` already hold `key` (a `a[b]` path)?
fn map_has_key_path(hash: &ParamMap, key: &str) -> bool {
    if key.contains("[]") {
        return false;
    }
    let mut parts = key.split(['[', ']']).filter(|p| !p.is_empty());
    let Some(first) = parts.next() else {
        return true;
    };
    let Some(mut current) = hash.get(first) else {
        return false;
    };
    for part in parts {
        match current {
            ParamValue::Map(map) => match map.get(part) {
                Some(next) => current = next,
                None => return false,
            },
            _ => return false,
        }
    }
    true
}

// --- JSON ----------------------------------------------------------------

fn from_json(value: serde_json::Value) -> ParamValue {
    use serde_json::Value;
    match value {
        Value::Null => ParamValue::Nil,
        Value::Bool(b) => ParamValue::Bool(b),
        Value::Number(n) => ParamValue::Num(n),
        Value::String(s) => ParamValue::Str(s),
        // Deep munge removes nils from arrays, at every depth.
        Value::Array(items) => ParamValue::Array(
            items
                .into_iter()
                .map(from_json)
                .filter(|v| !v.is_blank_slot())
                .collect(),
        ),
        Value::Object(object) => {
            let mut map = ParamMap::new();
            for (key, value) in object {
                map.insert(key, from_json(value));
            }
            ParamValue::Map(map)
        }
    }
}

/// Ruby's JSON parser treats `/* … */` and `// …` comments outside strings as
/// whitespace (observed on the Rails stack); `serde_json` does not. Returns
/// `None` for an unterminated block comment.
fn strip_json_comments(body: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if !body.contains(&b'/') {
        return Some(body.into());
    }
    let mut out = Vec::with_capacity(body.len());
    let (mut i, mut in_string, mut escaped) = (0, false, false);
    while i < body.len() {
        let b = body[i];
        if in_string {
            out.push(b);
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            i += 1;
        } else if b == b'"' {
            in_string = true;
            out.push(b);
            i += 1;
        } else if b == b'/' && body.get(i + 1) == Some(&b'*') {
            let end = body[i + 2..].windows(2).position(|w| w == b"*/")?;
            out.push(b' ');
            i += 2 + end + 2;
        } else if b == b'/' && body.get(i + 1) == Some(&b'/') {
            out.push(b' ');
            i += body[i..]
                .iter()
                .position(|c| *c == b'\n')
                .unwrap_or(body.len() - i);
        } else {
            out.push(b);
            i += 1;
        }
    }
    Some(out.into())
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(object) => 1 + object.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn to_json(value: &ParamValue) -> Value {
        match value {
            ParamValue::Nil => Value::Null,
            ParamValue::Str(s) => Value::String(s.clone()),
            ParamValue::Num(n) => Value::Number(n.clone()),
            ParamValue::Bool(b) => Value::Bool(*b),
            ParamValue::Array(items) => Value::Array(items.iter().map(to_json).collect()),
            ParamValue::Map(map) => map_to_json(map),
        }
    }

    fn map_to_json(map: &ParamMap) -> Value {
        Value::Object(
            map.iter()
                .map(|(k, v)| (k.to_owned(), to_json(v)))
                .collect(),
        )
    }

    fn parse(qs: &str) -> Value {
        map_to_json(RackParams::parse(qs).unwrap().map())
    }

    /// Pre-auth DoS guard: `insert`/`get` must be amortised O(1), so a query
    /// of many distinct keys parses in linear time (it was quadratic: 80 000
    /// keys took ~4.9 s in release). The bound is deliberately generous.
    #[test]
    fn many_distinct_keys_parse_in_linear_time_and_keep_insertion_order() {
        const KEYS: usize = 80_000;
        let query = (0..KEYS)
            .map(|i| format!("k{i}=1"))
            .collect::<Vec<_>>()
            .join("&");
        let started = std::time::Instant::now();
        let params = RackParams::parse(&query).unwrap();
        let elapsed = started.elapsed();
        eprintln!("parse of {KEYS} distinct keys: {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "{KEYS} keys took {elapsed:?}"
        );
        assert_eq!(params.map().len(), KEYS);
        let keys: Vec<&str> = params.map().iter().map(|(k, _)| k).collect();
        assert_eq!(keys[0], "k0");
        assert_eq!(keys[KEYS - 1], format!("k{}", KEYS - 1));
        assert_eq!(params.str("k79999"), Some("1"));
    }

    #[test]
    fn an_overwritten_key_keeps_its_original_position() {
        let mut map = ParamMap::new();
        map.insert("a".into(), ParamValue::Str("1".into()));
        map.insert("b".into(), ParamValue::Str("2".into()));
        map.insert("a".into(), ParamValue::Str("3".into()));
        let seen: Vec<_> = map.iter().map(|(k, v)| (k, v.as_str())).collect();
        assert_eq!(seen, [("a", Some("3")), ("b", Some("2"))]);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn nested_tags() {
        assert_eq!(
            parse("tags[type]=facture&tags[urgent]"),
            json!({"tags": {"type": "facture", "urgent": null}})
        );
    }

    #[test]
    fn array_limit_is_present_but_not_a_string() {
        let params = RackParams::parse("limit[]=1").unwrap();
        assert!(params.get("limit").is_some());
        assert_eq!(params.str("limit"), None);
        assert_eq!(RackParams::parse("limit[a]=1").unwrap().str("limit"), None);
    }

    #[test]
    fn duplicates_last_wins_empty_and_absent() {
        let params = RackParams::parse("limit=5&limit=7&offset=&flag").unwrap();
        assert_eq!(params.str("limit"), Some("7"));
        assert_eq!(params.str("offset"), Some(""));
        assert_eq!(params.str("flag"), None, "nil is not a string");
        assert_eq!(params.str("missing"), None);
        assert!(params.get("flag").is_some());
        assert!(params.get("missing").is_none());
    }

    #[test]
    fn type_conflicts_and_bad_encodings_are_rejected() {
        for qs in [
            "a=1&a[b]=2",
            "a[]=1&a[b]=2",
            "a=%zz",
            "a=%",
            "%ff=1",
            "a=%ff",
            "a[]=%ff",
        ] {
            assert!(RackParams::parse(qs).is_err(), "{qs}");
        }
        let mut deep = String::from("a");
        deep.push_str(&"[x]".repeat(100));
        assert!(RackParams::parse(&deep).is_err());
        let mut ok = String::from("a");
        ok.push_str(&"[x]".repeat(99));
        assert!(RackParams::parse(&ok).is_ok());
    }

    #[test]
    fn rejections_map_to_the_generic_400() {
        let error: ApiError = RackParams::parse("a=%zz").unwrap_err().into();
        assert_eq!(error, ApiError::BadRequest("bad request".into()));
    }

    /// The oracle: `ActionDispatch::ParamBuilder` itself, run on the Rails
    /// stack by `tests/fixtures/gen_rack_params_corpus.rb`.
    #[test]
    fn matches_rails_param_builder_on_the_corpus() {
        let corpus: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/rack_params_corpus.json"))
                .unwrap();
        assert!(corpus.len() > 100, "corpus looks truncated");
        let mut failures = Vec::new();
        for entry in &corpus {
            let raw = entry["raw"].as_str().unwrap();
            let got = RackParams::parse(raw).map(|p| map_to_json(p.map()));
            let want = &entry["result"];
            let ok = match (&got, want.get("ok")) {
                (Ok(g), Some(w)) => g == w,
                (Err(_), None) => want.get("error").is_some(),
                _ => false,
            };
            if !ok {
                let shown: String = raw.chars().take(60).collect();
                failures.push(format!("{shown:?}: got {got:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn json_object_body() {
        let params = RackParams::parse_json(
            br#"{"name":"x","parent_id":8,"tags":{"type":"f"},"ids":[1,null,2]}"#,
        )
        .unwrap();
        assert_eq!(params.str("name"), Some("x"));
        assert_eq!(params.str("parent_id"), None, "an integer is not a string");
        assert_eq!(
            map_to_json(params.map())["ids"],
            json!([1, 2]),
            "nil dropped from arrays"
        );
        assert_eq!(map_to_json(params.map())["parent_id"], json!(8));
    }

    #[test]
    fn json_comments_are_whitespace_like_in_ruby() {
        let p = RackParams::parse_json(b"{\"name\": /* a */ \"x\" // tail\n}").unwrap();
        assert_eq!(p.str("name"), Some("x"));
        let p = RackParams::parse_json(br#"{"name":"/* not a comment */ // nor this"}"#).unwrap();
        assert_eq!(p.str("name"), Some("/* not a comment */ // nor this"));
        let p = RackParams::parse_json(br#"{"name":"a\"/*b*/"}"#).unwrap();
        assert_eq!(p.str("name"), Some("a\"/*b*/"));
        assert_eq!(
            RackParams::parse_json(b"{} /* open"),
            Err(ParamsError::MalformedJson)
        );
        assert_eq!(
            RackParams::parse_json(b"{} / x"),
            Err(ParamsError::MalformedJson)
        );
    }

    #[test]
    fn json_scalar_and_array_bodies_become_underscore_json() {
        let params = RackParams::parse_json(b"[1,2]").unwrap();
        assert_eq!(map_to_json(params.map()), json!({"_json": [1, 2]}));
        let params = RackParams::parse_json(b"null").unwrap();
        assert_eq!(map_to_json(params.map()), json!({"_json": null}));
        let params = RackParams::parse_json(b"\"hi\"").unwrap();
        assert_eq!(params.str("_json"), Some("hi"));
    }

    #[test]
    fn malformed_json_is_an_error() {
        for body in [&b"{bad"[..], b"", b" ", b"{\"a\":", b"{\"a\":1,}", b"NaN"] {
            assert_eq!(
                RackParams::parse_json(body),
                Err(ParamsError::MalformedJson),
                "{body:?}"
            );
        }
        let nested = format!("{}{}", "[".repeat(101), "]".repeat(101));
        assert_eq!(
            RackParams::parse_json(nested.as_bytes()),
            Err(ParamsError::MalformedJson)
        );
        let ok = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(RackParams::parse_json(ok.as_bytes()).is_ok());
    }

    #[test]
    fn query_wins_over_body_on_a_clash() {
        let body = RackParams::parse("a=body&b=body").unwrap();
        let query = RackParams::parse("a=query").unwrap();
        let merged = RackParams::merged(body, query);
        assert_eq!(merged.str("a"), Some("query"));
        assert_eq!(merged.str("b"), Some("body"));
    }

    #[test]
    fn content_types() {
        use axum::http::HeaderValue;
        let ct = |v: &'static str| Some(HeaderValue::from_static(v));
        let kind = |m: &Method, v: Option<HeaderValue>| BodyKind::of(m, v.as_ref());
        assert_eq!(
            kind(&Method::POST, ct("Application/JSON; charset=utf-8")),
            BodyKind::Json
        );
        assert_eq!(kind(&Method::POST, ct("text/x-json")), BodyKind::Json);
        assert_eq!(
            kind(&Method::POST, ct("application/vnd.api+json")),
            BodyKind::Other
        );
        assert_eq!(
            kind(&Method::POST, ct("application/x-www-form-urlencoded")),
            BodyKind::Form
        );
        assert_eq!(kind(&Method::POST, None), BodyKind::Form);
        assert_eq!(kind(&Method::PUT, None), BodyKind::Other);
        assert_eq!(
            kind(&Method::POST, ct("multipart/form-data; boundary=x")),
            BodyKind::Other
        );
    }
}
