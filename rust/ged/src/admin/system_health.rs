//! Green/orange/red checks for the status page: port of
//! `app/services/system_health.rb`. Every check is local (database, worker
//! heartbeats, cached JWKS state, `df`) — never live HTTP on page load — and
//! a failing subsystem is reported, never raised.

use std::path::Path;
use std::time::Duration;

use sqlx::PgPool;

use super::disk_space::DiskSpace;
use crate::auth::JwksCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Ok,
    Warn,
    Bad,
    Unknown,
}

impl State {
    /// The word shown in the badge.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Bad => "bad",
            Self::Unknown => "unknown",
        }
    }

    /// `AdminHelper::HEALTH_BADGE`.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Bad => "bad",
            Self::Unknown => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub state: State,
    pub detail: String,
}

pub const DISK_WARN: f64 = 0.80;
pub const DISK_BAD: f64 = 0.90;

pub async fn checks(
    db: &PgPool,
    jwks: &JwksCache,
    storage_root: &Path,
    alive_threshold: Duration,
) -> Vec<Check> {
    vec![
        database(db).await,
        job_workers(db, alive_threshold).await,
        jwks_check(jwks),
        storage_disk(storage_root).await,
    ]
}

pub async fn database(db: &PgPool) -> Check {
    match sqlx::query("SELECT 1").execute(db).await {
        Ok(_) => Check {
            name: "Database",
            state: State::Ok,
            detail: "reachable".into(),
        },
        Err(_) => Check {
            name: "Database",
            state: State::Bad,
            detail: "SqlxError".into(),
        },
    }
}

pub async fn job_workers(db: &PgPool, threshold: Duration) -> Check {
    let row: Result<(i64, i64), sqlx::Error> = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE last_heartbeat_at > now() - make_interval(secs => $1)), \
                count(*) FROM job_workers",
    )
    .bind(threshold.as_secs_f64())
    .fetch_one(db)
    .await;
    match row {
        Ok((alive, _)) if alive > 0 => Check {
            name: "Job workers",
            state: State::Ok,
            detail: format!("{alive} alive"),
        },
        Ok((_, total)) if total > 0 => Check {
            name: "Job workers",
            state: State::Bad,
            detail: "heartbeats stale".into(),
        },
        Ok(_) => Check {
            name: "Job workers",
            state: State::Bad,
            detail: "no worker registered".into(),
        },
        Err(_) => Check {
            name: "Job workers",
            state: State::Bad,
            detail: "SqlxError".into(),
        },
    }
}

/// Staleness alone is normal (the cache refreshes lazily, on API traffic);
/// only a recorded refresh failure means the IdP is unreachable.
pub fn jwks_check(jwks: &JwksCache) -> Check {
    let health = jwks.health();
    if !health.fetched {
        Check {
            name: "OIDC JWKS",
            state: State::Unknown,
            detail: "not fetched yet (no API traffic)".into(),
        }
    } else if health.failing {
        Check {
            name: "OIDC JWKS",
            state: State::Warn,
            detail: format!(
                "serving stale keys, last refresh failed (age {}s)",
                health.age_seconds.unwrap_or(0.0) as u64
            ),
        }
    } else {
        Check {
            name: "OIDC JWKS",
            state: State::Ok,
            detail: "keys cached".into(),
        }
    }
}

pub fn disk_check(disk: Option<DiskSpace>) -> Check {
    let Some(disk) = disk else {
        return Check {
            name: "Storage disk",
            state: State::Unknown,
            detail: "df unavailable".into(),
        };
    };
    let used = disk.used_fraction();
    let state = if used >= DISK_BAD {
        State::Bad
    } else if used >= DISK_WARN {
        State::Warn
    } else {
        State::Ok
    };
    Check {
        name: "Storage disk",
        state,
        detail: format!("{}% used", (used * 100.0).round() as i64),
    }
}

pub async fn storage_disk(root: &Path) -> Check {
    let root = root.to_owned();
    let disk = tokio::task::spawn_blocking(move || DiskSpace::for_path(&root))
        .await
        .unwrap_or(None);
    disk_check(disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(total: u64, available: u64) -> Option<DiskSpace> {
        Some(DiskSpace {
            total_kb: total,
            available_kb: available,
        })
    }

    #[test]
    fn the_disk_thresholds_are_80_and_90_percent() {
        for (available, state, detail) in [
            (100, State::Ok, "0% used"),
            (21, State::Ok, "79% used"),
            (20, State::Warn, "80% used"),
            (11, State::Warn, "89% used"),
            (10, State::Bad, "90% used"),
            (0, State::Bad, "100% used"),
        ] {
            let c = disk_check(disk(100, available));
            assert_eq!((c.state, c.detail.as_str()), (state, detail), "{available}");
        }
    }

    #[test]
    fn no_df_is_unknown() {
        let c = disk_check(None);
        assert_eq!(
            (c.state, c.detail.as_str()),
            (State::Unknown, "df unavailable")
        );
    }

    #[test]
    fn the_jwks_line_is_one_of_three_states_and_never_fetches() {
        // An unreachable URL: a check that fetched would hang or fail.
        let cache = JwksCache::new("http://127.0.0.1:1/jwks".into());
        let c = jwks_check(&cache);
        assert_eq!(c.state, State::Unknown);
        assert_eq!(c.detail, "not fetched yet (no API traffic)");
    }

    #[test]
    fn badges_match_the_helper() {
        assert_eq!(State::Unknown.badge(), "info");
        assert_eq!(State::Warn.badge(), "warn");
        assert_eq!(State::Ok.as_str(), "ok");
    }
}
