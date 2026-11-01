//! JSON assertion helpers. Error messages are compared character for
//! character: the API contract includes the messages Rails generates.

use serde_json::{Value, json};

use crate::http::Resp;

/// Assert `status` and that the body is exactly `{"error": <message>}`.
#[track_caller]
pub fn assert_error(resp: &Resp, status: u16, message: &str) {
    assert_status(resp, status);
    assert_json_content_type(resp);
    assert_eq!(
        resp.json(),
        json!({ "error": message }),
        "unexpected error body (raw: {:?})",
        resp.text()
    );
}

#[track_caller]
pub fn assert_status(resp: &Resp, status: u16) {
    assert_eq!(
        resp.status,
        status,
        "unexpected status; body: {:?}",
        resp.text()
    );
}

#[track_caller]
pub fn assert_json_content_type(resp: &Resp) {
    let ct = resp.header("content-type").unwrap_or_default();
    assert!(
        ct.starts_with("application/json"),
        "content-type is {ct:?}, expected JSON"
    );
}

/// Assert the body equals `expected` exactly.
#[track_caller]
pub fn assert_json_eq(resp: &Resp, expected: &Value) {
    assert_eq!(&resp.json(), expected, "raw: {:?}", resp.text());
}

/// Assert every field of `subset` is present with the same value in `actual`
/// (objects are compared recursively; arrays and scalars exactly).
#[track_caller]
pub fn assert_json_includes(actual: &Value, subset: &Value) {
    assert!(
        includes(actual, subset),
        "{actual} does not include {subset}"
    );
}

fn includes(actual: &Value, subset: &Value) -> bool {
    match (actual, subset) {
        (Value::Object(a), Value::Object(s)) => s
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|av| includes(av, v))),
        _ => actual == subset,
    }
}
