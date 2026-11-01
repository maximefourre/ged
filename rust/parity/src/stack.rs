//! Coordinates of the running Docker stack (one per worktree) and a thin
//! wrapper around `bin/worktree-stack exec`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Stack {
    pub root: PathBuf,
    pub base_url: String,
    pub oauth_url: String,
    pub database_url: String,
}

/// Repository root (this crate lives in `<root>/rust/parity`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The stack of this worktree, resolved once per process.
pub fn stack() -> &'static Stack {
    static STACK: OnceLock<Stack> = OnceLock::new();
    STACK.get_or_init(resolve)
}

fn resolve() -> Stack {
    let root = repo_root();
    let (mut web, mut oauth, mut db) = (None, None, None);
    if env_nonempty("GED_BASE_URL").is_none()
        || env_nonempty("GED_OAUTH_URL").is_none()
        || env_nonempty("DATABASE_URL").is_none()
    {
        // `▶ project=ged-x  web=3109  oauth=9401  db=5541`
        let out = Command::new(root.join("bin/worktree-stack"))
            .arg("urls")
            .current_dir(&root)
            .output()
            .expect("run bin/worktree-stack urls");
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next().unwrap_or_default();
        let field = |name: &str| {
            line.split_whitespace()
                .find_map(|w| w.strip_prefix(&format!("{name}=")))
                .map(str::to_owned)
        };
        web = field("web");
        oauth = field("oauth");
        db = field("db");
    }
    let base_url = env_nonempty("GED_BASE_URL")
        .or_else(|| web.map(|p| format!("http://localhost:{p}")))
        .expect("GED_BASE_URL unset and bin/worktree-stack urls gave no web port");
    let oauth_url = env_nonempty("GED_OAUTH_URL")
        .or_else(|| oauth.map(|p| format!("http://localhost:{p}")))
        .expect("GED_OAUTH_URL unset and bin/worktree-stack urls gave no oauth port");
    let database_url = env_nonempty("DATABASE_URL")
        .or_else(|| {
            // Rails compose uses `ged_production`. `GED_DATABASE_NAME` lets a
            // future backend point the same fallback at its own database
            // without changing that default.
            db.map(|p| {
                format!(
                    "postgres://postgres:postgres@localhost:{p}/{}",
                    database_name()
                )
            })
        })
        .expect("DATABASE_URL unset and bin/worktree-stack urls gave no db port");
    Stack {
        root,
        base_url: base_url.trim_end_matches('/').to_owned(),
        oauth_url: oauth_url.trim_end_matches('/').to_owned(),
        database_url,
    }
}

/// Run a command inside a compose service of this worktree's stack
/// (`bin/worktree-stack exec -T <service> <args…>`), returning stdout.
pub fn exec(service: &str, args: &[&str]) -> Result<String, String> {
    let s = stack();
    let out = Command::new(s.root.join("bin/worktree-stack"))
        .args(["exec", "-T", service])
        .args(args)
        .current_dir(&s.root)
        .output()
        .map_err(|e| format!("spawn bin/worktree-stack: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "`exec {service} {}` failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Business database name used when `DATABASE_URL` is unset.
///
/// Default `ged_production` matches this repo's compose file. Set
/// `GED_DATABASE_NAME` for a backend whose database is named differently.
/// An explicit `DATABASE_URL` still wins, unchanged.
pub fn database_name() -> String {
    env_nonempty("GED_DATABASE_NAME").unwrap_or_else(|| "ged_production".into())
}

/// Admin credentials: `GED_ADMIN_USERNAME` (default `admin`) and
/// `GED_ADMIN_PASSWORD`, falling back to the worktree's `.env`.
pub fn admin_credentials() -> (String, String) {
    let user = env_nonempty("GED_ADMIN_USERNAME").unwrap_or_else(|| "admin".into());
    let pass = env_nonempty("GED_ADMIN_PASSWORD")
        .or_else(|| {
            let env = std::fs::read_to_string(stack().root.join(".env")).ok()?;
            env.lines()
                .find_map(|l| l.strip_prefix("GED_ADMIN_PASSWORD="))
                .map(|v| v.trim().trim_matches('"').to_owned())
        })
        .expect("GED_ADMIN_PASSWORD unset and absent from .env");
    (user, pass)
}
