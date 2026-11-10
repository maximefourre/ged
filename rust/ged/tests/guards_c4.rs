//! Port of `test/integration/c4_diagrams_test.rb`.
//!
//! Diagrams that go stale are worse than no diagrams. Compose services, Rust
//! job kinds, Rails service objects (while `app/services` is still here) and
//! the queue pools must all appear in `docs/architecture/c4.md`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn diagram() -> String {
    fs::read_to_string(root().join("docs/architecture/c4.md")).expect("read c4.md")
}

fn camelize_segment(segment: &str) -> String {
    segment
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// `auth/jwks_cache` → `Auth::JwksCache` (Rails `String#camelize` on a path).
fn camelize_path(rel: &str) -> String {
    rel.split('/')
        .map(camelize_segment)
        .collect::<Vec<_>>()
        .join("::")
}

fn ruby_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(ruby_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rb") {
            out.push(path);
        }
    }
    out
}

#[test]
fn camelize_matches_rails_for_service_paths() {
    assert_eq!(camelize_path("auth/jwks_cache"), "Auth::JwksCache");
    assert_eq!(camelize_path("callback_url_policy"), "CallbackUrlPolicy");
    assert_eq!(camelize_path("store_document_job"), "StoreDocumentJob");
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
    let mut missing: Vec<String> = registry
        .kinds()
        .into_iter()
        .filter(|kind| kind.ends_with("Job") && !doc.contains(kind))
        .map(str::to_owned)
        .collect();

    let jobs_dir = root().join("app/jobs");
    if jobs_dir.is_dir() {
        for path in ruby_files(&jobs_dir) {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem == "application_job" {
                continue;
            }
            let name = camelize_segment(stem);
            if !doc.contains(&name) {
                missing.push(name);
            }
        }
    }

    assert!(missing.is_empty(), "jobs absent from c4.md: {missing:?}");
}

#[test]
fn every_service_object_appears_in_a_component_diagram() {
    let services = root().join("app/services");
    if !services.is_dir() {
        return;
    }
    let doc = diagram();
    let mut missing = Vec::new();
    for path in ruby_files(&services) {
        let rel = path
            .strip_prefix(&services)
            .expect("under app/services")
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let name = camelize_path(&rel);
        if !doc.contains(&name) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "service objects absent from c4.md: {missing:?}"
    );
}

#[test]
fn every_recurring_task_class_is_drawn_as_scheduled() {
    let doc = diagram();
    let mut missing = Vec::new();

    let schedule = root().join("config/recurring.yml");
    if schedule.is_file() {
        let parsed: Value = serde_yaml::from_str(&fs::read_to_string(&schedule).unwrap())
            .expect("parse recurring.yml");
        if let Some(tasks) = parsed.get("production").and_then(Value::as_mapping) {
            for task in tasks.values() {
                if let Some(class) = task.get("class").and_then(Value::as_str)
                    && !doc.contains(class)
                {
                    missing.push(class.to_owned());
                }
            }
        }
    }

    for task in ged::jobs::scheduler::default_schedule() {
        if task.kind.ends_with("Job") && !doc.contains(&task.kind) {
            missing.push(task.kind);
        }
    }

    assert!(
        missing.is_empty(),
        "recurring jobs absent from c4.md: {missing:?}"
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
