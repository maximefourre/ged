//! Diagrams that go stale are worse than no diagrams.
//!
//! `docs/architecture/c4.md` must name every Docker Compose service, every
//! registered job kind, every Rust module that replaced an `app/services`
//! object, every scheduled task, and every worker pool. The lists come from
//! `docker-compose.yml` and the `ged` crate, not from a Ruby tree.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn diagram() -> String {
    fs::read_to_string(root().join("docs/architecture/c4.md")).expect("read c4.md")
}

/// `jwks_cache` → `JwksCache`.
fn camelize(stem: &str) -> String {
    stem.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Name `c4.md` must contain for a service module. A few Rust filenames do
/// not match the component the diagram cites; a new file uses [`camelize`].
fn service_diagram_name(rel: &str) -> String {
    match rel {
        "policy/callback_url.rs" => "CallbackUrlPolicy".to_owned(),
        "storage/staging.rs" => "StagingStore".to_owned(),
        "admin/zip_stream.rs" => "DocumentZipper".to_owned(),
        other => {
            let stem = Path::new(other)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(other);
            camelize(stem)
        }
    }
}

/// Rust modules drawn as services. `mod.rs` is a barrel.
/// `auth/authenticated_client.rs` is the request principal, grouped with the
/// models and deliberately not drawn on its own.
///
/// `auth/` and `policy/` are scanned. The other paths are an explicit list:
/// a new file under `storage/`, `jobs/`, `domain/` or `admin/` is invisible
/// until it is added here. `docs/architecture/c4.md` records that limit.
fn service_modules() -> Vec<String> {
    let src = root().join("rust/ged/src");
    let mut rels = Vec::new();
    for rel in [
        "domain/document_creator.rs",
        "storage/staging.rs",
        "admin/disk_space.rs",
        "admin/system_health.rs",
        "admin/zip_stream.rs",
        "telemetry.rs",
    ] {
        rels.push(rel.to_owned());
    }
    for dir in ["auth", "policy"] {
        let dir_path = src.join(dir);
        for entry in fs::read_dir(&dir_path).unwrap_or_else(|_| panic!("read {dir}")) {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "mod.rs" || name == "authenticated_client.rs" || !name.ends_with(".rs") {
                continue;
            }
            rels.push(format!("{dir}/{name}"));
        }
    }
    rels.sort();
    rels
}

#[test]
fn camelize_matches_the_service_module_names() {
    assert_eq!(camelize("jwks_cache"), "JwksCache");
    assert_eq!(camelize("disk_space"), "DiskSpace");
    assert_eq!(camelize("document_creator"), "DocumentCreator");
    assert_eq!(
        service_diagram_name("policy/callback_url.rs"),
        "CallbackUrlPolicy"
    );
}

#[test]
fn the_diagram_file_exists_and_every_mermaid_block_is_closed() {
    let path = root().join("docs/architecture/c4.md");
    assert!(path.is_file(), "docs/architecture/c4.md is missing");
    let doc = diagram();
    let opened = doc.lines().filter(|line| *line == "```mermaid").count();
    let fences = doc.lines().filter(|line| *line == "```").count();
    assert_eq!(opened, fences, "unbalanced mermaid fences in c4.md");
    assert!(opened >= 7, "expected at least 7 diagrams, found {opened}");
}

#[test]
fn every_docker_compose_service_appears_in_the_container_diagram() {
    let compose: Value =
        serde_yaml::from_str(&fs::read_to_string(root().join("docker-compose.yml")).unwrap())
            .expect("parse docker-compose.yml");
    let services = compose
        .get("services")
        .and_then(Value::as_mapping)
        .expect("services");
    let doc = diagram();
    let missing: Vec<_> = services
        .keys()
        .filter_map(Value::as_str)
        .filter(|name| !doc.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "compose services absent from c4.md: {missing:?}"
    );
}

#[tokio::test]
async fn every_job_kind_appears_in_the_worker_diagram() {
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@localhost/unused")
        .expect("lazy pool");
    let env = ged::jobs::env::PipelineEnv::from_config(db, None);
    let registry =
        ged::jobs::worker::registry(&env, ged::jobs::deliver_callback::production_policy());
    let doc = diagram();
    let missing: Vec<String> = registry
        .kinds()
        .into_iter()
        .filter(|kind| !doc.contains(kind))
        .map(str::to_owned)
        .collect();
    assert!(missing.is_empty(), "jobs absent from c4.md: {missing:?}");
}

#[test]
fn every_service_module_appears_in_a_component_diagram() {
    let src = root().join("rust/ged/src");
    let doc = diagram();
    let mut missing = Vec::new();
    for rel in service_modules() {
        let path = src.join(&rel);
        assert!(path.is_file(), "service module missing: {rel}");
        let name = service_diagram_name(&rel);
        if !doc.contains(&name) {
            missing.push(format!("{rel} ({name})"));
        }
    }
    assert!(
        missing.is_empty(),
        "service modules absent from c4.md: {missing:?}"
    );
}

#[test]
fn every_scheduled_task_is_drawn() {
    let doc = diagram();
    let missing: Vec<_> = ged::jobs::scheduler::default_schedule()
        .into_iter()
        .filter(|task| !doc.contains(&task.kind))
        .map(|task| task.kind)
        .collect();
    assert!(
        missing.is_empty(),
        "scheduled tasks absent from c4.md: {missing:?}"
    );
}

#[test]
fn every_queue_pool_is_documented() {
    let doc = diagram();
    let settings = ged::jobs::worker::WorkerSettings::from_lookup(&|_| None);
    let mut missing = Vec::new();
    for runner in settings.runners() {
        let needle = format!("Pool: {}", runner.queue.as_str());
        if !doc.contains(&needle) {
            missing.push(needle);
        }
    }
    assert!(
        missing.is_empty(),
        "queue pools absent from c4.md: {missing:?}"
    );
}
