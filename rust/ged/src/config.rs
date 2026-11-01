//! Process configuration, read from the same environment variables as the
//! Rails app, with the same fail-closed boot rules.
//!
//! Port of `config/initializers/oidc_config.rb` (`OidcConfig.verify!`): in
//! production the process refuses to start when API bearer auth cannot work;
//! in development it only warns so the admin UI stays usable without an IdP;
//! in test nothing is checked.

use std::fmt;

/// `GED_MAX_UPLOAD_BYTES` default: 50 MiB.
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 52_428_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Test,
    Production,
}

impl Environment {
    /// `RAILS_ENV`, kept as the variable name so compose files do not change.
    fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("production") => Self::Production,
            Some("test") => Self::Test,
            _ => Self::Development,
        }
    }
}

/// How a token is bound to GED (`GED_OIDC_TOKEN_BINDING`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenBinding {
    /// `aud` must match `GED_OIDC_AUDIENCE`.
    Audience,
    /// `scope`/`scp` must carry `GED_OIDC_REQUIRED_SCOPE`.
    Scope,
    /// Any other value: bearer auth can never succeed.
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    pub issuer: Option<String>,
    pub jwks_url: Option<String>,
    pub binding: TokenBinding,
    pub audience: Option<String>,
    pub required_scope: Option<String>,
    /// `GED_CLIENT_ID_CLAIM`, default `sub`.
    pub client_id_claim: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub environment: Environment,
    pub database_url: Option<String>,
    pub secret_key_base: Option<String>,
    pub oidc: OidcConfig,
    pub admin_username: String,
    pub admin_password: Option<String>,
    pub max_upload_bytes: u64,
    /// Non-fatal problems found at boot (development only).
    pub warnings: Vec<String>,
}

/// The process must not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Ruby `String#present?`.
fn present(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Ruby `String#to_i`: leading integer, `0` when there is none.
fn ruby_to_i(raw: &str) -> u64 {
    let trimmed = raw.trim_start();
    let digits: String = trimmed
        .strip_prefix('+')
        .unwrap_or(trimmed)
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .collect();
    digits.replace('_', "").parse().unwrap_or(0)
}

impl Config {
    pub fn from_process_env() -> Result<Self, ConfigError> {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// `lookup` stands in for `ENV[]`, so tests never touch the process env.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let environment = Environment::parse(lookup("RAILS_ENV").as_deref());
        let get = |key: &str| present(lookup(key));

        let binding = match lookup("GED_OIDC_TOKEN_BINDING").as_deref() {
            None | Some("audience") => TokenBinding::Audience,
            Some("scope") => TokenBinding::Scope,
            Some(other) => TokenBinding::Invalid(other.to_owned()),
        };
        let oidc = OidcConfig {
            issuer: get("GED_OIDC_ISSUER"),
            jwks_url: get("GED_OIDC_JWKS_URL"),
            binding,
            audience: get("GED_OIDC_AUDIENCE"),
            required_scope: get("GED_OIDC_REQUIRED_SCOPE"),
            client_id_claim: lookup("GED_CLIENT_ID_CLAIM").unwrap_or_else(|| "sub".into()),
        };

        let mut config = Self {
            environment,
            database_url: get("DATABASE_URL"),
            secret_key_base: get("SECRET_KEY_BASE"),
            oidc,
            admin_username: lookup("GED_ADMIN_USERNAME").unwrap_or_else(|| "admin".into()),
            admin_password: lookup("GED_ADMIN_PASSWORD"),
            max_upload_bytes: lookup("GED_MAX_UPLOAD_BYTES")
                .map_or(DEFAULT_MAX_UPLOAD_BYTES, |v| ruby_to_i(&v)),
            warnings: Vec::new(),
        };
        config.verify()?;
        Ok(config)
    }

    fn verify(&mut self) -> Result<(), ConfigError> {
        if self.environment == Environment::Test {
            return Ok(());
        }
        let mut problems = Vec::new();
        let oidc = &self.oidc;

        let mut missing = Vec::new();
        if oidc.issuer.is_none() {
            missing.push("GED_OIDC_ISSUER");
        }
        if oidc.jwks_url.is_none() {
            missing.push("GED_OIDC_JWKS_URL");
        }
        match &oidc.binding {
            TokenBinding::Audience if oidc.audience.is_none() => missing.push("GED_OIDC_AUDIENCE"),
            TokenBinding::Scope if oidc.required_scope.is_none() => {
                missing.push("GED_OIDC_REQUIRED_SCOPE")
            }
            TokenBinding::Invalid(other) => problems.push(format!(
                "Invalid GED_OIDC_TOKEN_BINDING={other} (must be 'audience' or 'scope')"
            )),
            _ => {}
        }
        if !missing.is_empty() {
            problems.push(format!("Missing OIDC config: {}", missing.join(", ")));
        }

        let mut message = (!problems.is_empty())
            .then(|| format!("{} — API bearer auth cannot work", problems.join("; ")));

        if self.environment == Environment::Production && self.secret_key_base.is_none() {
            let secret = "Missing SECRET_KEY_BASE — sessions cannot be signed".to_owned();
            message = Some(match message {
                Some(m) => format!("{m}; {secret}"),
                None => secret,
            });
        }

        match (message, self.environment) {
            (Some(m), Environment::Production) => Err(ConfigError(m)),
            (Some(m), _) => {
                self.warnings.push(m);
                Ok(())
            }
            (None, _) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        Config::from_lookup(&|k| map.get(k).cloned())
    }

    const PROD_BASE: &[(&str, &str)] = &[
        ("RAILS_ENV", "production"),
        ("SECRET_KEY_BASE", "s3cret"),
        ("GED_OIDC_ISSUER", "http://idp"),
        ("GED_OIDC_JWKS_URL", "http://idp/jwks"),
    ];

    fn prod_with(extra: &[(&'static str, &'static str)]) -> Result<Config, ConfigError> {
        let mut all = PROD_BASE.to_vec();
        all.extend_from_slice(extra);
        load(&all)
    }

    #[test]
    fn production_refuses_to_boot_without_an_audience() {
        let err = prod_with(&[]).unwrap_err();
        assert_eq!(
            err.0,
            "Missing OIDC config: GED_OIDC_AUDIENCE — API bearer auth cannot work"
        );
    }

    #[test]
    fn production_boots_with_an_audience() {
        let config = prod_with(&[("GED_OIDC_AUDIENCE", "ged")]).unwrap();
        assert_eq!(config.oidc.binding, TokenBinding::Audience);
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn scope_binding_requires_a_scope_not_an_audience() {
        let err = prod_with(&[
            ("GED_OIDC_TOKEN_BINDING", "scope"),
            ("GED_OIDC_AUDIENCE", "ged"),
        ])
        .unwrap_err();
        assert_eq!(
            err.0,
            "Missing OIDC config: GED_OIDC_REQUIRED_SCOPE — API bearer auth cannot work"
        );
        let ok = prod_with(&[
            ("GED_OIDC_TOKEN_BINDING", "scope"),
            ("GED_OIDC_REQUIRED_SCOPE", "urn:x:ged"),
        ])
        .unwrap();
        assert_eq!(ok.oidc.binding, TokenBinding::Scope);
    }

    #[test]
    fn blank_values_count_as_missing() {
        let err = prod_with(&[("GED_OIDC_AUDIENCE", "   ")]).unwrap_err();
        assert!(err.0.contains("GED_OIDC_AUDIENCE"), "{err}");
    }

    #[test]
    fn unknown_binding_is_refused_and_named() {
        let err = prod_with(&[
            ("GED_OIDC_TOKEN_BINDING", "both"),
            ("GED_OIDC_AUDIENCE", "ged"),
        ])
        .unwrap_err();
        assert_eq!(
            err.0,
            "Invalid GED_OIDC_TOKEN_BINDING=both (must be 'audience' or 'scope') — API bearer auth cannot work"
        );
    }

    #[test]
    fn every_missing_variable_is_listed_in_order() {
        let err = load(&[("RAILS_ENV", "production"), ("SECRET_KEY_BASE", "x")]).unwrap_err();
        assert_eq!(
            err.0,
            "Missing OIDC config: GED_OIDC_ISSUER, GED_OIDC_JWKS_URL, GED_OIDC_AUDIENCE — API bearer auth cannot work"
        );
    }

    #[test]
    fn production_requires_a_secret_key_base() {
        let mut all = PROD_BASE.to_vec();
        all.retain(|(k, _)| *k != "SECRET_KEY_BASE");
        all.push(("GED_OIDC_AUDIENCE", "ged"));
        assert!(load(&all).unwrap_err().0.contains("SECRET_KEY_BASE"));
    }

    #[test]
    fn development_only_warns() {
        let config = load(&[]).unwrap();
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].starts_with("Missing OIDC config: GED_OIDC_ISSUER"));
    }

    #[test]
    fn test_environment_is_not_checked() {
        let config = load(&[("RAILS_ENV", "test")]).unwrap();
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn defaults_follow_rails() {
        let config = load(&[("RAILS_ENV", "test")]).unwrap();
        assert_eq!(config.oidc.client_id_claim, "sub");
        assert_eq!(config.admin_username, "admin");
        assert_eq!(config.max_upload_bytes, 52_428_800);
        let config = load(&[
            ("RAILS_ENV", "test"),
            ("GED_MAX_UPLOAD_BYTES", "1024"),
            ("GED_CLIENT_ID_CLAIM", "azp"),
        ])
        .unwrap();
        assert_eq!(config.max_upload_bytes, 1024);
        assert_eq!(config.oidc.client_id_claim, "azp");
    }

    #[test]
    fn upload_limit_parses_like_ruby_to_i() {
        for (raw, want) in [
            ("12abc", 12),
            ("abc", 0),
            (" 7", 7),
            ("", 0),
            ("1_000", 1000),
        ] {
            let config = load(&[("RAILS_ENV", "test"), ("GED_MAX_UPLOAD_BYTES", raw)]).unwrap();
            assert_eq!(config.max_upload_bytes, want, "{raw:?}");
        }
    }
}
