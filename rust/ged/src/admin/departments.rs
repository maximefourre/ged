//! `Admin::DepartmentsController`: the tree, create, rename, and the delete
//! flow (hard delete, or the resolution page: move everything elsewhere, or
//! trash the department with its documents).

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use sqlx::PgPool;

use super::view::{AdminError, AdminResult, Chrome, Ctx, rails_id};
use crate::domain::department::{Department, DepartmentError};
use crate::http::params::{ParamMap, RackParams};
use crate::http::{AppState, RequestParams};

/// Non-breaking spaces, three per level, as the Rails helper prints them.
pub fn indent(levels: usize) -> String {
    "\u{a0}".repeat(3 * levels)
}

/// Depth-first `(department, depth)` pairs, names sorted at each level.
/// Works on any subset: a node whose parent is not in the set is a root, so a
/// filtered set stays complete (`Department.tree`). Iterative, so a very deep
/// tree costs no stack.
pub fn tree(nodes: Vec<Department>) -> Vec<(Department, usize)> {
    let ids: HashSet<i64> = nodes.iter().map(|d| d.id).collect();
    let mut by_parent: HashMap<i64, Vec<Department>> = HashMap::new();
    let mut roots = Vec::new();
    for d in nodes {
        match d.parent_id {
            Some(p) if ids.contains(&p) => by_parent.entry(p).or_default().push(d),
            _ => roots.push(d),
        }
    }
    let sorted = |mut v: Vec<Department>| {
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    };
    let mut out = Vec::new();
    // Reversed so that popping yields the smallest name first.
    let mut stack: Vec<(Department, usize)> =
        sorted(roots).into_iter().rev().map(|d| (d, 0)).collect();
    while let Some((dept, depth)) = stack.pop() {
        let children = by_parent.remove(&dept.id).map(sorted).unwrap_or_default();
        out.push((dept, depth));
        stack.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
    }
    out
}

/// One `<option>`: indented label, id, selected?
pub struct TreeOption {
    pub label: String,
    pub id: i64,
    pub selected: bool,
}

pub fn tree_options(pairs: &[(Department, usize)], selected: Option<i64>) -> Vec<TreeOption> {
    pairs
        .iter()
        .map(|(d, depth)| TreeOption {
            label: format!("{}{}", indent(*depth), d.name),
            id: d.id,
            selected: selected == Some(d.id),
        })
        .collect()
}

async fn all_departments(db: &PgPool) -> Result<Vec<Department>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, parent_id, created_at, updated_at, deleted_at FROM departments",
    )
    .fetch_all(db)
    .await
}

pub async fn live_departments(db: &PgPool) -> Result<Vec<Department>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, parent_id, created_at, updated_at, deleted_at \
         FROM departments WHERE deleted_at IS NULL",
    )
    .fetch_all(db)
    .await
}

async fn counts(db: &PgPool, trashed: bool) -> Result<HashMap<i64, i64>, sqlx::Error> {
    let sql = if trashed {
        "SELECT department_id, count(*) FROM documents WHERE deleted_at IS NOT NULL GROUP BY 1"
    } else {
        "SELECT department_id, count(*) FROM documents WHERE deleted_at IS NULL GROUP BY 1"
    };
    let rows: Vec<(i64, i64)> = sqlx::query_as(sql).fetch_all(db).await?;
    Ok(rows.into_iter().collect())
}

// --- index ----------------------------------------------------------------

struct Row {
    id: i64,
    name: String,
    parent_id: Option<i64>,
    trashed: bool,
    depth: usize,
    has_children: bool,
    documents: i64,
}

impl Row {
    fn indent(&self) -> String {
        indent(self.depth.saturating_sub(1))
    }
}

#[derive(Template)]
#[template(path = "admin/departments/index.html")]
struct IndexPage {
    chrome: Chrome,
    rows: Vec<Row>,
}

pub async fn index(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let pairs = tree(all_departments(&state.db).await?);
    let parents: HashSet<i64> = pairs.iter().filter_map(|(d, _)| d.parent_id).collect();
    let live = counts(&state.db, false).await?;
    let trashed = counts(&state.db, true).await?;
    let rows = pairs
        .into_iter()
        .map(|(d, depth)| {
            let is_trashed = d.deleted_at.is_some();
            let documents = if is_trashed { &trashed } else { &live };
            Row {
                documents: documents.get(&d.id).copied().unwrap_or(0),
                has_children: parents.contains(&d.id),
                id: d.id,
                parent_id: d.parent_id,
                trashed: is_trashed,
                depth,
                name: d.name,
            }
        })
        .collect();
    Ok(ctx.page(
        StatusCode::OK,
        &IndexPage {
            chrome: ctx.chrome("departments"),
            rows,
        },
    ))
}

// --- new / create ----------------------------------------------------------

#[derive(Template)]
#[template(path = "admin/departments/form.html")]
struct FormPage {
    chrome: Chrome,
    /// `None` for a new department, its id for a rename.
    id: Option<i64>,
    name: String,
    parents: Vec<TreeOption>,
}

impl FormPage {
    fn title(&self) -> &'static str {
        if self.id.is_some() {
            "Rename department"
        } else {
            "New department"
        }
    }
}

async fn parent_options(
    db: &PgPool,
    selected: Option<i64>,
) -> Result<Vec<TreeOption>, sqlx::Error> {
    Ok(tree_options(&tree(live_departments(db).await?), selected))
}

pub async fn new(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let page = FormPage {
        chrome: ctx.chrome("departments"),
        id: None,
        name: String::new(),
        parents: parent_options(&state.db, None).await?,
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

/// `params.require(key)`: a non-blank value, here always a hash of fields.
pub(super) fn require<'a>(params: &'a RackParams, key: &str) -> Result<&'a ParamMap, AdminError> {
    match params.get(key) {
        Some(crate::http::params::ParamValue::Map(m)) if !m.is_empty() => Ok(m),
        // A scalar under the key would reach `permit` as a String in Rails: a 500.
        Some(crate::http::params::ParamValue::Str(s)) if !s.trim().is_empty() => Err(
            AdminError::Internal(format!("params[{key}] is a string, not a hash")),
        ),
        _ => Err(AdminError::BadRequest),
    }
}

/// Ruby `String#blank?`.
pub(super) fn blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// `name` must be present and unique among siblings, trashed ones included.
async fn name_errors(
    db: &PgPool,
    name: &str,
    parent_id: Option<i64>,
    except: Option<i64>,
) -> Result<Vec<String>, sqlx::Error> {
    let mut errors = Vec::new();
    if blank(name) {
        errors.push("Name can't be blank".to_owned());
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM departments WHERE name = $1 \
         AND parent_id IS NOT DISTINCT FROM $2 AND ($3::bigint IS NULL OR id <> $3))",
    )
    .bind(name)
    .bind(parent_id)
    .bind(except)
    .fetch_one(db)
    .await?;
    if taken {
        errors.push("Name has already been taken".to_owned());
    }
    Ok(errors)
}

fn unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|d| d.is_unique_violation())
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let fields = require(&params, "department")?;
    let name = fields
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let raw_parent = fields
        .get("parent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parent_id = if blank(raw_parent) {
        None
    } else {
        Some(rails_id(raw_parent).unwrap_or(0))
    };

    // The parent must exist (a trashed one still does, as for the foreign
    // key); Rails fails on the insert with a 500.
    if let Some(p) = parent_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE id = $1)")
                .bind(p)
                .fetch_one(&state.db)
                .await?;
        if !exists {
            return Err(AdminError::Internal(format!(
                "parent department {p} does not exist"
            )));
        }
    }

    let errors = name_errors(&state.db, &name, parent_id, None).await?;
    if errors.is_empty() {
        let inserted = sqlx::query(
            "INSERT INTO departments (name, parent_id, created_at, updated_at) \
             VALUES ($1, $2, now() AT TIME ZONE 'utc', now() AT TIME ZONE 'utc')",
        )
        .bind(&name)
        .bind(parent_id)
        .execute(&state.db)
        .await;
        match inserted {
            Ok(_) => {
                ctx.flash("notice", "Department created.");
                return Ok(ctx.redirect("/admin/departments"));
            }
            // Two creations of the same name at once: the index decides.
            Err(e) if unique_violation(&e) => {
                return rerender_new(
                    &state,
                    &ctx,
                    name,
                    parent_id,
                    vec!["Name has already been taken".into()],
                )
                .await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    rerender_new(&state, &ctx, name, parent_id, errors).await
}

async fn rerender_new(
    state: &AppState,
    ctx: &Ctx,
    name: String,
    parent_id: Option<i64>,
    errors: Vec<String>,
) -> AdminResult {
    ctx.flash_now("alert", errors.join(", "));
    let page = FormPage {
        chrome: ctx.chrome("departments"),
        id: None,
        name,
        parents: parent_options(&state.db, parent_id).await?,
    };
    Ok(ctx.page(StatusCode::UNPROCESSABLE_ENTITY, &page))
}

// --- edit / update ---------------------------------------------------------

async fn live(db: &PgPool, raw: &str) -> Result<Department, AdminError> {
    let id = rails_id(raw).ok_or(AdminError::NotFound)?;
    Department::find(db, id).await?.ok_or(AdminError::NotFound)
}

async fn any_state(db: &PgPool, raw: &str) -> Result<Department, AdminError> {
    let id = rails_id(raw).ok_or(AdminError::NotFound)?;
    Department::find_with_deleted(db, id)
        .await?
        .ok_or(AdminError::NotFound)
}

pub async fn edit(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let dept = live(&state.db, &id).await?;
    let page = FormPage {
        chrome: ctx.chrome("departments"),
        id: Some(dept.id),
        name: dept.name,
        parents: Vec::new(),
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

/// Rename only: re-parenting would silently move a whole subtree across the
/// permission boundary.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let dept = live(&state.db, &id).await?;
    let fields = require(&params, "department")?;
    // `permit(:name)`: without a name the record keeps its own.
    let name = fields
        .get("name")
        .and_then(|v| v.as_str())
        .map_or_else(|| dept.name.clone(), str::to_owned);
    let errors = name_errors(&state.db, &name, dept.parent_id, Some(dept.id)).await?;
    if errors.is_empty() {
        let updated = sqlx::query(
            "UPDATE departments SET name = $1, updated_at = now() AT TIME ZONE 'utc' WHERE id = $2",
        )
        .bind(&name)
        .bind(dept.id)
        .execute(&state.db)
        .await;
        match updated {
            Ok(_) => {
                ctx.flash("notice", "Department renamed.");
                return Ok(ctx.redirect("/admin/departments"));
            }
            Err(e) if !unique_violation(&e) => return Err(e.into()),
            Err(_) => {}
        }
    }
    let errors = if errors.is_empty() {
        vec!["Name has already been taken".to_owned()]
    } else {
        errors
    };
    ctx.flash_now("alert", errors.join(", "));
    let page = FormPage {
        chrome: ctx.chrome("departments"),
        id: Some(dept.id),
        name,
        parents: Vec::new(),
    };
    Ok(ctx.page(StatusCode::UNPROCESSABLE_ENTITY, &page))
}

// --- delete flow -----------------------------------------------------------

pub async fn destroy(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let dept = any_state(&state.db, &id).await?;
    let has_children: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)")
            .bind(dept.id)
            .fetch_one(&state.db)
            .await?;
    if has_children {
        ctx.flash(
            "alert",
            format!(
                "Cannot delete {}: it has sub-departments. Delete or move them first.",
                dept.name
            ),
        );
        return Ok(ctx.redirect("/admin/departments"));
    }
    // Trashed documents count: only a purge may remove their department.
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM documents WHERE department_id = $1) \
             OR EXISTS (SELECT 1 FROM upload_requests WHERE department_id = $1)",
    )
    .bind(dept.id)
    .fetch_one(&state.db)
    .await?;
    if referenced {
        return Ok(ctx.redirect(&format!("/admin/departments/{}/confirm_deletion", dept.id)));
    }
    match dept.purge(&state.db).await {
        Ok(()) => ctx.flash("notice", "Department deleted."),
        Err(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation()) =>
        {
            ctx.flash(
                "alert",
                "Department is still referenced by other records and cannot be deleted.",
            )
        }
        Err(e) => return Err(e.into()),
    }
    Ok(ctx.redirect("/admin/departments"))
}

struct Target {
    options: Vec<TreeOption>,
}

#[derive(Template)]
#[template(path = "admin/departments/confirm_deletion.html")]
struct ConfirmPage {
    chrome: Chrome,
    id: i64,
    name: String,
    live_docs: i64,
    trashed_docs: i64,
    pending_requests: i64,
    target: Target,
}

pub async fn confirm_deletion(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let dept = any_state(&state.db, &id).await?;
    let live_docs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE department_id = $1 AND deleted_at IS NULL",
    )
    .bind(dept.id)
    .fetch_one(&state.db)
    .await?;
    let trashed_docs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE department_id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(dept.id)
    .fetch_one(&state.db)
    .await?;
    let pending_requests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM upload_requests WHERE department_id = $1")
            .bind(dept.id)
            .fetch_one(&state.db)
            .await?;
    let others: Vec<Department> = live_departments(&state.db)
        .await?
        .into_iter()
        .filter(|d| d.id != dept.id)
        .collect();
    let pairs = tree(others);
    // The live parent, when it is still a possible target.
    let default_target = dept
        .parent_id
        .filter(|p| pairs.iter().any(|(d, _)| d.id == *p));
    let page = ConfirmPage {
        chrome: ctx.chrome("departments"),
        id: dept.id,
        name: dept.name,
        live_docs,
        trashed_docs,
        pending_requests,
        target: Target {
            options: tree_options(&pairs, default_target),
        },
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

pub async fn move_documents(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let source = any_state(&state.db, &id).await?;
    let has_children: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)")
            .bind(source.id)
            .fetch_one(&state.db)
            .await?;
    if has_children {
        ctx.flash(
            "alert",
            format!("Cannot move: {} has sub-departments.", source.name),
        );
        return Ok(ctx.redirect("/admin/departments"));
    }
    // `Department.find(params[:target_department_id])`: live only, 404 otherwise.
    let target_id = params
        .str("target_department_id")
        .and_then(rails_id)
        .ok_or(AdminError::NotFound)?;
    let target = Department::find(&state.db, target_id)
        .await?
        .ok_or(AdminError::NotFound)?;
    if target.id == source.id {
        ctx.flash("alert", "Choose a different destination department.");
        return Ok(ctx.redirect(&format!(
            "/admin/departments/{}/confirm_deletion",
            source.id
        )));
    }
    let mut tx = state.db.begin().await?;
    for table in ["documents", "upload_requests"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET department_id = $1, updated_at = now() AT TIME ZONE 'utc' \
             WHERE department_id = $2"
        )))
        .bind(target.id)
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM client_department_accesses WHERE department_id = $1")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM departments WHERE id = $1")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    ctx.flash(
        "notice",
        format!(
            "Moved documents to {} and deleted {}.",
            target.name, source.name
        ),
    );
    Ok(ctx.redirect("/admin/departments"))
}

pub async fn trash(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let dept = live(&state.db, &id).await?;
    match dept.soft_delete(&state.db).await {
        Ok(()) => {}
        // Rails lets `RecordNotDestroyed` through: a 500.
        Err(DepartmentError::NotALeaf) => {
            return Err(AdminError::Internal("Department is not a leaf".into()));
        }
        Err(DepartmentError::Db(e)) => return Err(e.into()),
    }
    ctx.flash(
        "notice",
        format!(
            "{} moved to trash. It will be removed automatically once its documents are purged.",
            dept.name
        ),
    );
    Ok(ctx.redirect("/admin/departments"))
}

pub async fn restore(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let dept = any_state(&state.db, &id).await?;
    if dept.deleted_at.is_none() {
        return Err(AdminError::NotFound);
    }
    dept.restore(&state.db).await?;
    ctx.flash("notice", format!("{} restored.", dept.name));
    Ok(ctx.redirect("/admin/departments"))
}

#[allow(dead_code)]
fn _response(_: Response) {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    fn dept(id: i64, name: &str, parent: Option<i64>) -> Department {
        let t = NaiveDateTime::default();
        Department {
            id,
            name: name.into(),
            parent_id: parent,
            created_at: t,
            updated_at: t,
            deleted_at: None,
        }
    }

    #[test]
    fn the_tree_is_depth_first_with_names_sorted_per_level() {
        let pairs = tree(vec![
            dept(1, "b", None),
            dept(2, "a", None),
            dept(3, "z", Some(2)),
            dept(4, "y", Some(2)),
            dept(5, "c", Some(4)),
        ]);
        let got: Vec<(&str, usize)> = pairs.iter().map(|(d, n)| (d.name.as_str(), *n)).collect();
        assert_eq!(got, [("a", 0), ("y", 1), ("c", 2), ("z", 1), ("b", 0)]);
    }

    #[test]
    fn an_orphaned_subtree_becomes_a_root() {
        // The parent (9) is not in the set (a filtered view).
        let pairs = tree(vec![dept(1, "kid", Some(9)), dept(2, "top", None)]);
        let got: Vec<(&str, usize)> = pairs.iter().map(|(d, n)| (d.name.as_str(), *n)).collect();
        assert_eq!(got, [("kid", 0), ("top", 0)]);
    }

    #[test]
    fn a_corrupt_cycle_is_left_out_not_looped_on() {
        let pairs = tree(vec![
            dept(1, "a", Some(2)),
            dept(2, "b", Some(1)),
            dept(3, "ok", None),
        ]);
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn a_very_deep_tree_does_not_overflow_the_stack() {
        let nodes: Vec<Department> = (1..=50_000)
            .map(|i| dept(i, &format!("d{i}"), (i > 1).then_some(i - 1)))
            .collect();
        assert_eq!(tree(nodes).len(), 50_000);
    }

    #[test]
    fn indentation_is_three_non_breaking_spaces_per_level() {
        assert_eq!(indent(2), "\u{a0}".repeat(6));
        assert_eq!(indent(0), "");
    }
}
