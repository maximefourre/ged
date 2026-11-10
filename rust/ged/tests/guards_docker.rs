//! T14: the default image and compose file run Rust. Rails stays selectable.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn default_dockerfile_serves_ged_and_rails_image_is_kept() {
    let dockerfile = read("Dockerfile");
    assert!(
        dockerfile.contains(r#"CMD ["ged", "serve"]"#),
        "Dockerfile must default to `ged serve`"
    );
    let rails = read("Dockerfile.rails");
    assert!(
        rails.contains("bin/rails") && rails.contains("thrust"),
        "Dockerfile.rails must still build the Rails image"
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

    let overlay = read("docker-compose.rails.yml");
    assert!(overlay.contains("Dockerfile.rails"));
    assert!(overlay.contains(r#""./bin/jobs""#));
    assert!(overlay.contains("bin/rails"));
}

#[test]
fn worktree_stack_defaults_to_rust_and_selects_rails() {
    let script = read("bin/worktree-stack");
    assert!(script.contains("GED_STACK"));
    assert!(script.contains("docker-compose.rails.yml"));
    assert!(
        script.contains("ged seed"),
        "seed must run ged seed on the Rust stack"
    );
    assert!(
        script.contains("bin/rails db:seed"),
        "Rails seed stays available"
    );
}

#[test]
fn ci_still_drives_rails_and_checks_rust() {
    let bin = read("bin/ci");
    assert!(
        bin.contains("config/ci.rb"),
        "bin/ci still loads config/ci.rb"
    );
    let ci = read("config/ci.rb");
    assert!(ci.contains("bin/rails test"), "Rails tests stay until T16");
    assert!(ci.contains("cargo test"), "Rust tests stay in bin/ci");
    assert!(
        ci.contains("GED_STACK=rails"),
        "bin/ci must validate the Rails compose overlay"
    );
    let workflow = read(".github/workflows/ci.yml");
    assert!(
        workflow.contains("cargo test -p ged"),
        "GitHub CI must run the Rust test suite"
    );
    assert!(
        workflow.contains("cargo clippy"),
        "GitHub CI must lint Rust"
    );
}
