//! The `authenticate_app_client!` before-action as an extractor: a handler
//! that takes an [`AuthenticatedClient`] only runs for a valid bearer token of
//! a registered client.

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::domain::app_client::{AccessMode, AppClient};
use crate::http::{ApiError, AppState};

#[derive(Debug, Clone)]
pub struct AuthenticatedClient(pub AppClient);

impl AuthenticatedClient {
    pub fn client(&self) -> &AppClient {
        &self.0
    }

    /// `authorize_department!`: 403 `no <mode> access to this department`.
    pub async fn authorize_department(
        &self,
        state: &AppState,
        department_id: i64,
        mode: AccessMode,
    ) -> Result<(), ApiError> {
        match self.0.can_access(&state.db, department_id, mode).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(ApiError::Forbidden(format!(
                "no {} access to this department",
                mode.as_str()
            ))),
            Err(e) => {
                eprintln!("ged: authorization query failed: {e}");
                Err(ApiError::Internal)
            }
        }
    }
}

/// Rails matches `\ABearer (.+)\z` on the header and then drops a blank
/// token: any other scheme, an empty or whitespace-only token, is "missing".
/// Repeated headers reach Rails joined by `", "`.
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let mut joined = String::new();
    for (i, value) in headers.get_all(AUTHORIZATION).iter().enumerate() {
        if i > 0 {
            joined.push_str(", ");
        }
        joined.push_str(value.to_str().ok()?);
    }
    let token = joined.strip_prefix("Bearer ")?;
    (!token.contains('\n') && !token.trim().is_empty()).then(|| token.to_owned())
}

impl FromRequestParts<AppState> for AuthenticatedClient {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;

        let validated = match state.token_validator().call(&token).await {
            Ok(v) => v,
            Err(e) => {
                // The reason stays in the log: never leak jwt/JWKS internals,
                // or the attacker-controlled kid, in the response.
                eprintln!("ged: bearer auth rejected: {e}");
                return Err(ApiError::Unauthorized("invalid token".into()));
            }
        };
        match AppClient::find_by_identifier(&state.db, &validated.client_identifier).await {
            Ok(Some(client)) => Ok(Self(client)),
            Ok(None) => Err(ApiError::Forbidden("client not registered".into())),
            Err(e) => {
                eprintln!("ged: app client lookup failed: {e}");
                Err(ApiError::Internal)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(AUTHORIZATION, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn only_a_bearer_scheme_with_a_non_blank_token_counts() {
        assert_eq!(
            bearer_token(&headers(&["Bearer abc"])).as_deref(),
            Some("abc")
        );
        assert_eq!(
            bearer_token(&headers(&["Bearer  a b"])).as_deref(),
            Some(" a b")
        );
        for bad in [
            "bearer abc",
            "Bearer",
            "Bearer ",
            "Bearer    ",
            "Basic abc",
            "Bearerabc",
        ] {
            assert_eq!(bearer_token(&headers(&[bad])), None, "{bad:?}");
        }
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn repeated_headers_are_joined_like_the_rack_env() {
        assert_eq!(
            bearer_token(&headers(&["Bearer a", "Bearer b"])).as_deref(),
            Some("a, Bearer b")
        );
    }

    #[test]
    fn a_non_text_header_is_a_missing_token() {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_bytes(b"Bearer \xff\xfe").unwrap(),
        );
        assert_eq!(bearer_token(&h), None);
    }
}
