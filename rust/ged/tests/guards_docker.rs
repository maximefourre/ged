//! The image and compose file run Rust. There is no Rails image or overlay.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn default_dockerfile_serves_ged() {
    let dockerfile = read("Dockerfile");
    assert!(
        dockerfile.contains(r#"CMD ["ged", "serve"]"#),
        "Dockerfile must default to `ged serve`"
    );
    assert!(
        !root().join("Dockerfile.rails").exists(),
        "Dockerfile.rails was removed in T16; replay Rails from the tag rails-final"
    );
}

#[test]
fn compose_runs_one_rust_image_for_web_and_worker() {
    let compose = read("docker-compose.yml");
    assert!(
        compose.contains(r#"command: ["ged", "serve"]"#),
        "web must run ged serve"
    );
    assert!(
        compose.contains(r#"command: ["ged", "worker"]"#),
        "worker must run ged worker"
    );
    assert!(
        compose.contains("stop_grace_period: 40s"),
        "worker stop_grace_period stays 40s"
    );
    assert!(compose.contains("/up"), "healthcheck must probe /up");
    assert!(
        compose.contains(r#""${GED_WEB_PORT:-3000}:80""#),
        "published web port stays host:80"
    );
    assert!(
        compose.contains("storage:/rails/storage"),
        "web and worker keep the shared storage volume"
    );
    assert!(
        !root().join("docker-compose.rails.yml").exists(),
        "docker-compose.rails.yml was removed in T16"
    );
}

#[test]
fn worktree_stack_is_rust_only() {
    let script = read("bin/worktree-stack");
    assert!(script.contains("GED_STACK"));
    assert!(
        !script.contains("docker-compose.rails.yml"),
        "the stack script must not select a Rails overlay"
    );
    assert!(script.contains("ged seed"), "seed must run ged seed");
    assert!(!script.contains("bin/rails"), "bin/rails is gone");
}

#[test]
fn ci_checks_rust_and_skips_parity_without_a_stack() {
    let bin = read("bin/ci");
    assert!(
        !bin.contains("bin/rails") && !bin.contains("config/ci.rb"),
        "bin/ci must not drive Rails"
    );
    assert!(
        !root().join("config/ci.rb").exists(),
        "config/ci.rb is gone"
    );
    assert!(bin.contains("cargo fmt --check"), "fmt stays");
    assert!(
        bin.contains("cargo clippy") && bin.contains("-D warnings"),
        "clippy denies warnings"
    );
    assert!(
        bin.contains("cargo test -p ged"),
        "Rust tests run the ged package, not the whole workspace"
    );
    assert!(
        !bin.contains("cargo test\""),
        "a workspace-wide cargo test would also run parity"
    );
    assert!(
        bin.contains("bundle exec rake"),
        "oauth-provider tests stay"
    );
    assert!(
        bin.contains("playwright"),
        "Playwright runs against a live stack"
    );
    assert!(
        bin.contains("cargo test -p parity") && bin.contains("GED_BACKEND=rust"),
        "parity is its own step and targets Rust"
    );
    assert!(
        !bin.contains("GED_BACKEND=rails") && !bin.contains("GED_STACK=rails"),
        "Rails is not a backend bin/ci can select"
    );
    assert!(
        bin.contains("Parity: skipped"),
        "a stack that does not answer must be an explicit skip"
    );
    assert!(
        bin.contains("bin/worktree-stack config"),
        "compose config stays"
    );
    assert!(
        bin.contains("docker build -t ged:ci .") && !bin.contains("Dockerfile.rails"),
        "bin/ci builds the Rust image only"
    );
    assert!(
        bin.contains("docker info"),
        "the image build runs only when Docker answers"
    );
    assert!(
        bin.contains("ruby -c") && bin.contains("bin/perf-upload") && bin.contains("bin/monkey"),
        "the remaining Ruby drivers are syntax-checked"
    );
    assert!(
        bin.contains("cargo audit"),
        "Rust dependencies are audited; bundler-audit left with the Rails Gemfile"
    );
    assert!(
        bin.contains("bundle-audit") && bin.contains("oauth-provider/Gemfile.lock"),
        "oauth-provider gems stay under bundle-audit"
    );
    let workflow = read(".github/workflows/ci.yml");
    assert!(
        workflow.contains("cargo test -p ged"),
        "GitHub CI runs the Rust suite"
    );
    assert!(workflow.contains("cargo clippy"), "GitHub CI lints Rust");
    assert!(
        workflow.contains("docker build -t ged:ci ."),
        "GitHub CI builds the Rust image"
    );
    assert!(
        workflow.contains("bundle exec rake"),
        "GitHub CI tests oauth-provider"
    );
    assert!(!workflow.contains("Dockerfile.rails") && !workflow.contains("bin/rails"));
    assert!(
        workflow.contains("cargo audit"),
        "GitHub CI audits Rust dependencies"
    );
    assert!(
        workflow.contains("bundle-audit") && workflow.contains("oauth-provider/Gemfile.lock"),
        "GitHub CI audits oauth-provider gems"
    );
}

#[test]
fn images_run_as_non_root_and_bake_no_secrets() {
    let file = "Dockerfile";
    let text = read(file);
    let user = last_user(&text).unwrap_or_else(|| panic!("{file} sets no USER"));
    assert!(is_non_root(&user), "{file} USER {user} must not be root");
    let secrets = secret_env_or_arg(&text);
    assert!(
        secrets.is_empty(),
        "{file} ARG/ENV bakes secrets: {}",
        secrets.join(", ")
    );
}

#[test]
fn secret_detector_flags_env_and_arg_names() {
    let text = "ENV PORT=80 \\\n    RAILS_ENV=production\nARG SECRET_KEY_BASE=dev\nRUN SECRET_KEY_BASE_DUMMY=1 true\n";
    assert_eq!(secret_env_or_arg(text), vec!["SECRET_KEY_BASE".to_owned()]);
    assert!(is_non_root("1000:1000"));
    assert!(!is_non_root("root"));
    assert!(!is_non_root("0:0"));
    assert!(last_user("FROM scratch\nUSER 1000:1000\n").as_deref() == Some("1000:1000"));
}

#[test]
fn worker_stop_grace_exceeds_the_shutdown_timeout_default() {
    let compose = read("docker-compose.yml");
    let grace = worker_stop_grace_secs(&compose).expect("worker stop_grace_period as a duration");
    let compose_default =
        compose_shutdown_default_secs(&compose).expect("SOLID_QUEUE_SHUTDOWN_TIMEOUT default");
    let code_default = rust_shutdown_default_secs(&read("rust/ged/src/jobs/worker.rs"))
        .expect("WorkerSettings default shutdown timeout");
    assert_eq!(
        compose_default, code_default,
        "compose default and the Rust default diverged"
    );
    assert!(
        grace > compose_default,
        "worker stop_grace_period ({grace}s) must be strictly greater than SOLID_QUEUE_SHUTDOWN_TIMEOUT default ({compose_default}s)"
    );
}

#[test]
fn grace_equal_to_the_shutdown_default_does_not_satisfy_the_invariant() {
    let sample = r#"
services:
  worker:
    environment:
      SOLID_QUEUE_SHUTDOWN_TIMEOUT: ${SOLID_QUEUE_SHUTDOWN_TIMEOUT:-40}
    stop_grace_period: 40s
"#;
    let grace = worker_stop_grace_secs(sample).unwrap();
    let default = compose_shutdown_default_secs(sample).unwrap();
    assert!(
        grace <= default,
        "the fixture must show a grace period that is not strictly greater"
    );
}

#[test]
fn rust_image_caches_cargo_outside_the_final_stage() {
    let text = read("Dockerfile");
    assert!(
        text.contains("cargo build --release --locked -p ged"),
        "the image must still compile ged"
    );
    assert!(
        text.contains("--mount=type=cache")
            && text.contains("target=/usr/local/cargo/registry")
            && text.contains("target=/src/rust/target"),
        "cargo registry and target must be BuildKit cache mounts"
    );
    let stage = final_stage(&text);
    assert!(
        !stage.to_ascii_lowercase().contains("cargo"),
        "final stage must not contain cargo:\n{stage}"
    );
    assert!(
        stage.contains("COPY --from=build /out/ged /usr/local/bin/ged"),
        "final stage copies only the binary that was lifted out of the cache"
    );
}

#[test]
fn clean_forgets_the_remembered_stack() {
    let arm = case_arm(&read("bin/worktree-stack"), "clean");
    assert!(
        arm.contains("rm -f \"$(stack_state_file)\""),
        "clean must remove the remembered stack file, arm was:\n{arm}"
    );
}

#[test]
fn worktree_stack_active_follows_env_then_memory_then_rust() {
    let tmp = std::env::temp_dir().join(format!(
        "ged-active-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp).unwrap();
    let state = tmp.join(format!("ged-worktree-stack-{}", worktree_slug()));

    let rails_env = run_active(&tmp, Some("rails"));
    assert!(!rails_env.status.success(), "GED_STACK=rails must fail");
    assert!(
        stderr(&rails_env).contains("expected rust"),
        "stderr was: {}",
        stderr(&rails_env)
    );

    let rust_env = run_active(&tmp, Some("rust"));
    assert!(rust_env.status.success(), "{}", stderr(&rust_env));
    assert_eq!(stdout(&rust_env), "rust\n");

    fs::write(&state, "rails\n").unwrap();
    let remembered = run_active(&tmp, None);
    assert!(
        !remembered.status.success(),
        "a remembered rails name must not be selected"
    );
    assert!(
        stderr(&remembered).contains("expected rust"),
        "stderr was: {}",
        stderr(&remembered)
    );

    let env_wins = run_active(&tmp, Some("rust"));
    assert_eq!(
        stdout(&env_wins),
        "rust\n",
        "GED_STACK=rust wins over the remembered name"
    );

    fs::remove_file(&state).unwrap();
    let fresh = run_active(&tmp, None);
    assert!(fresh.status.success(), "{}", stderr(&fresh));
    assert_eq!(stdout(&fresh), "rust\n");

    let bad = run_active(&tmp, Some("nope"));
    assert!(!bad.status.success(), "an unknown stack must fail");
    let err = stderr(&bad);
    assert!(err.contains("expected rust"), "stderr was: {err}");

    let _ = fs::remove_dir_all(&tmp);
}

fn worktree_slug() -> String {
    let canonical = root().canonicalize().unwrap_or_else(|_| root());
    let name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut dashed = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            dashed.push(c);
        } else {
            dashed.push('-');
        }
    }
    let mut squeezed = String::new();
    let mut prev_dash = false;
    for c in dashed.chars() {
        if c == '-' {
            if !prev_dash {
                squeezed.push('-');
            }
            prev_dash = true;
        } else {
            prev_dash = false;
            squeezed.push(c);
        }
    }
    squeezed.trim_matches('-').to_owned()
}

fn run_active(tmpdir: &std::path::Path, ged_stack: Option<&str>) -> std::process::Output {
    let mut cmd = std::process::Command::new(root().join("bin/worktree-stack"));
    cmd.arg("active")
        .env("TMPDIR", tmpdir)
        .env_remove("GED_STACK");
    if let Some(stack) = ged_stack {
        cmd.env("GED_STACK", stack);
    }
    cmd.output().expect("run bin/worktree-stack active")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim_end();
        if let Some(kept) = trimmed.strip_suffix('\\') {
            current.push_str(kept);
            current.push(' ');
        } else {
            current.push_str(line);
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn last_user(dockerfile: &str) -> Option<String> {
    let mut found = None;
    for line in logical_lines(dockerfile) {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("USER ") {
            found = Some(rest.trim().to_owned());
        }
    }
    found
}

fn is_non_root(user: &str) -> bool {
    let name = user.split(':').next().unwrap_or(user).trim();
    !name.is_empty() && name != "root" && name != "0"
}

fn secret_env_or_arg(dockerfile: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in logical_lines(dockerfile) {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed
            .strip_prefix("ENV ")
            .or_else(|| trimmed.strip_prefix("ARG "))
        else {
            continue;
        };
        for token in rest.split_whitespace() {
            let name = token.split('=').next().unwrap_or(token).trim_matches('"');
            if is_secret_name(name) {
                found.push(name.to_owned());
            }
        }
    }
    found
}

fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "TOKEN",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "API_KEY",
        "MASTER_KEY",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn compose_value(compose: &str) -> serde_yaml::Value {
    serde_yaml::from_str(compose).unwrap_or_else(|err| panic!("compose yaml: {err}"))
}

fn worker_stop_grace_secs(compose: &str) -> Option<u64> {
    let value = compose_value(compose);
    let raw = value
        .get("services")?
        .get("worker")?
        .get("stop_grace_period")?;
    let text = raw.as_str()?;
    let secs = text.strip_suffix('s')?;
    if secs.is_empty() || !secs.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    secs.parse().ok()
}

fn compose_shutdown_default_secs(compose: &str) -> Option<u64> {
    let value = compose_value(compose);
    let raw = value
        .get("services")?
        .get("worker")?
        .get("environment")?
        .get("SOLID_QUEUE_SHUTDOWN_TIMEOUT")?
        .as_str()?;
    let start = raw.find(":-")? + 2;
    let digits: String = raw[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn rust_shutdown_default_secs(source: &str) -> Option<u64> {
    // The doc comment also names the variable. The default that ships is the
    // `unwrap_or` next to the lookup in `WorkerSettings::from_lookup`.
    let key = source.find("lookup(\"SOLID_QUEUE_SHUTDOWN_TIMEOUT\")")?;
    let window = &source[key..(key + 400).min(source.len())];
    let marker = "unwrap_or(";
    let at = window.find(marker)? + marker.len();
    let digits: String = window[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn final_stage(dockerfile: &str) -> String {
    match dockerfile.rfind("\nFROM ") {
        Some(at) => dockerfile[at + 1..].to_owned(),
        None => dockerfile.to_owned(),
    }
}

fn case_arm(script: &str, name: &str) -> String {
    let needle = format!("\n  {name})");
    let Some(start) = script.find(&needle) else {
        return String::new();
    };
    let rest = &script[start + needle.len()..];
    let end = rest.find("\n    ;;").unwrap_or(rest.len());
    rest[..end].to_owned()
}
