//! `Admin::AppClientsController`: register API clients and edit their
//! per-department Read/Write grid.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use sqlx::PgPool;

use super::departments::{blank, indent, require, tree};
use super::view::{AdminError, AdminResult, Chrome, Ctx, rails_id};
use crate::domain::department::Department;
use crate::http::params::{ParamValue, RackParams};
use crate::http::{AppState, RequestParams};

#[derive(Debug, Clone, sqlx::FromRow)]
struct Client {
    id: i64,
    client_identifier: String,
    description: Option<String>,
}

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
struct Grant {
    app_client_id: i64,
    department_id: i64,
    can_read: bool,
    can_write: bool,
}

// --- index -----------------------------------------------------------------

struct Chip {
    label: String,
    flags: String,
}

struct IndexRow {
    id: i64,
    identifier: String,
    description: String,
    chips: Vec<Chip>,
}

#[derive(Template)]
#[template(path = "admin/app_clients/index.html")]
struct IndexPage {
    chrome: Chrome,
    rows: Vec<IndexRow>,
}

async fn departments_by_id(
    db: &PgPool,
    ids: &[i64],
) -> Result<HashMap<i64, Department>, sqlx::Error> {
    // Trashed departments included: their grants outlive the soft delete.
    let rows: Vec<Department> = sqlx::query_as(
        "SELECT id, name, parent_id, created_at, updated_at, deleted_at \
         FROM departments WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|d| (d.id, d)).collect())
}

pub async fn index(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let clients: Vec<Client> = sqlx::query_as(
        "SELECT id, client_identifier, description FROM app_clients ORDER BY client_identifier",
    )
    .fetch_all(&state.db)
    .await?;
    let grants: Vec<Grant> = sqlx::query_as(
        "SELECT app_client_id, department_id, can_read, can_write \
         FROM client_department_accesses ORDER BY id",
    )
    .fetch_all(&state.db)
    .await?;
    let dept_ids: Vec<i64> = grants.iter().map(|g| g.department_id).collect();
    let depts = departments_by_id(&state.db, &dept_ids).await?;
    let rows = clients
        .into_iter()
        .map(|c| IndexRow {
            chips: grants
                .iter()
                .filter(|g| g.app_client_id == c.id)
                .map(|g| {
                    let dept = depts.get(&g.department_id);
                    let mut label = dept.map(|d| d.name.clone()).unwrap_or_default();
                    if dept.is_some_and(|d| d.deleted_at.is_some()) {
                        label.push_str(" (trashed)");
                    }
                    let flags = format!(
                        "{}{}",
                        if g.can_read { "R" } else { "" },
                        if g.can_write { "W" } else { "" }
                    );
                    Chip { label, flags }
                })
                .collect(),
            id: c.id,
            identifier: c.client_identifier,
            description: c.description.unwrap_or_default(),
        })
        .collect();
    Ok(ctx.page(
        StatusCode::OK,
        &IndexPage {
            chrome: ctx.chrome("app_clients"),
            rows,
        },
    ))
}

// --- form ------------------------------------------------------------------

struct GridRow {
    id: i64,
    name: String,
    indent: String,
    deeper: bool,
    trashed: bool,
    read: bool,
    write: bool,
}

#[derive(Template)]
#[template(path = "admin/app_clients/form.html")]
struct FormPage {
    chrome: Chrome,
    /// `None` when registering.
    id: Option<i64>,
    identifier: String,
    description: String,
    grid: Vec<GridRow>,
}

impl FormPage {
    fn title(&self) -> &'static str {
        if self.id.is_some() {
            "Edit client"
        } else {
            "Register client"
        }
    }

    fn submit(&self) -> &'static str {
        if self.id.is_some() {
            "Update App client"
        } else {
            "Create App client"
        }
    }
}

/// Live departments plus any trashed one the client still holds a grant on,
/// depth first, with the client's current flags.
async fn grid_for(db: &PgPool, client: Option<i64>) -> Result<Vec<GridRow>, sqlx::Error> {
    let departments: Vec<Department> = sqlx::query_as(
        "SELECT id, name, parent_id, created_at, updated_at, deleted_at FROM departments \
         WHERE deleted_at IS NULL \
            OR id IN (SELECT department_id FROM client_department_accesses WHERE app_client_id = $1)",
    )
    .bind(client)
    .fetch_all(db)
    .await?;
    let grants: Vec<Grant> = sqlx::query_as(
        "SELECT app_client_id, department_id, can_read, can_write \
         FROM client_department_accesses WHERE app_client_id = $1",
    )
    .bind(client)
    .fetch_all(db)
    .await?;
    Ok(tree(departments)
        .into_iter()
        .map(|(d, depth)| {
            let grant = grants.iter().find(|g| g.department_id == d.id);
            GridRow {
                id: d.id,
                indent: indent(depth.saturating_sub(1)),
                deeper: depth > 0,
                trashed: d.deleted_at.is_some(),
                read: grant.is_some_and(|g| g.can_read),
                write: grant.is_some_and(|g| g.can_write),
                name: d.name,
            }
        })
        .collect())
}

pub async fn new(State(state): State<AppState>, Extension(ctx): Extension<Ctx>) -> AdminResult {
    let page = FormPage {
        chrome: ctx.chrome("app_clients"),
        id: None,
        identifier: String::new(),
        description: String::new(),
        grid: grid_for(&state.db, None).await?,
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

async fn find(db: &PgPool, raw: &str) -> Result<Client, AdminError> {
    let id = rails_id(raw).ok_or(AdminError::NotFound)?;
    sqlx::query_as("SELECT id, client_identifier, description FROM app_clients WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AdminError::NotFound)
}

pub async fn edit(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let client = find(&state.db, &id).await?;
    let page = FormPage {
        chrome: ctx.chrome("app_clients"),
        id: Some(client.id),
        identifier: client.client_identifier,
        description: client.description.unwrap_or_default(),
        grid: grid_for(&state.db, Some(client.id)).await?,
    };
    Ok(ctx.page(StatusCode::OK, &page))
}

// --- create / update -------------------------------------------------------

/// `app_client_params`: the permitted scalar fields that were submitted.
struct Fields {
    identifier: Option<String>,
    description: Option<String>,
}

fn fields(params: &RackParams) -> Result<Fields, AdminError> {
    let map = require(params, "app_client")?;
    let text = |key: &str| map.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Ok(Fields {
        identifier: text("client_identifier"),
        description: text("description"),
    })
}

/// `app_client[access][<department id>][read|write]`: ticked boxes send "1"
/// after a hidden "0". Keyed by the submitted id text.
fn desired_access(params: &RackParams) -> HashMap<String, (bool, bool)> {
    let Some(ParamValue::Map(client)) = params.get("app_client") else {
        return HashMap::new();
    };
    let Some(ParamValue::Map(access)) = client.get("access") else {
        return HashMap::new();
    };
    access
        .iter()
        .map(|(id, flags)| {
            let flag = |name: &str| {
                flags
                    .as_map()
                    .and_then(|m| m.get(name))
                    .and_then(|v| v.as_str())
                    == Some("1")
            };
            (id.to_owned(), (flag("read"), flag("write")))
        })
        .collect()
}

async fn identifier_errors(
    db: &PgPool,
    identifier: &str,
    except: Option<i64>,
) -> Result<Vec<String>, sqlx::Error> {
    let mut errors = Vec::new();
    if blank(identifier) {
        errors.push("Client identifier can't be blank".to_owned());
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM app_clients WHERE client_identifier = $1 \
         AND ($2::bigint IS NULL OR id <> $2))",
    )
    .bind(identifier)
    .bind(except)
    .fetch_one(db)
    .await?;
    if taken {
        errors.push("Client identifier has already been taken".to_owned());
    }
    Ok(errors)
}

/// Reconciles the submitted grid with the stored grants: a ticked box upserts
/// the row (in place, keeping its id), both boxes empty remove it. Unknown
/// department ids are ignored; trashed departments are looked up too so
/// their grants can be removed.
async fn sync_access(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    client: i64,
    desired: &HashMap<String, (bool, bool)>,
) -> Result<(), sqlx::Error> {
    let ids: Vec<i64> = desired.keys().filter_map(|k| rails_id(k)).collect();
    let known: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM departments WHERE id = ANY($1) ORDER BY id")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    for department in known {
        let Some(&(read, write)) = desired.get(&department.to_string()) else {
            continue;
        };
        if read || write {
            sqlx::query(
                "INSERT INTO client_department_accesses \
                 (app_client_id, department_id, can_read, can_write, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, now() AT TIME ZONE 'utc', now() AT TIME ZONE 'utc') \
                 ON CONFLICT (app_client_id, department_id) DO UPDATE \
                 SET can_read = $3, can_write = $4, updated_at = now() AT TIME ZONE 'utc'",
            )
            .bind(client)
            .bind(department)
            .bind(read)
            .bind(write)
            .execute(&mut **tx)
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM client_department_accesses WHERE app_client_id = $1 AND department_id = $2",
            )
            .bind(client)
            .bind(department)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let f = fields(&params)?;
    let identifier = f.identifier.unwrap_or_default();
    let description = f.description;
    let mut errors = identifier_errors(&state.db, &identifier, None).await?;
    if errors.is_empty() {
        let mut tx = state.db.begin().await?;
        let inserted: Result<i64, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO app_clients (client_identifier, description, created_at, updated_at) \
             VALUES ($1, $2, now() AT TIME ZONE 'utc', now() AT TIME ZONE 'utc') RETURNING id",
        )
        .bind(&identifier)
        .bind(&description)
        .fetch_one(&mut *tx)
        .await;
        match inserted {
            Ok(id) => {
                sync_access(&mut tx, id, &desired_access(&params)).await?;
                tx.commit().await?;
                ctx.flash("notice", "Client registered.");
                return Ok(ctx.redirect("/admin/app_clients"));
            }
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation()) =>
            {
                errors.push("Client identifier has already been taken".to_owned());
            }
            Err(e) => return Err(e.into()),
        }
    }
    ctx.flash_now("alert", errors.join(", "));
    let page = FormPage {
        chrome: ctx.chrome("app_clients"),
        id: None,
        identifier,
        description: description.unwrap_or_default(),
        grid: grid_for(&state.db, None).await?,
    };
    Ok(ctx.page(StatusCode::UNPROCESSABLE_ENTITY, &page))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
    RequestParams(params): RequestParams,
) -> AdminResult {
    let client = find(&state.db, &id).await?;
    let f = fields(&params)?;
    // `assign_attributes`: a field that was not submitted keeps its value.
    let identifier = f
        .identifier
        .unwrap_or_else(|| client.client_identifier.clone());
    let description = f.description.or_else(|| client.description.clone());
    let mut errors = identifier_errors(&state.db, &identifier, Some(client.id)).await?;
    if errors.is_empty() {
        let mut tx = state.db.begin().await?;
        let updated = sqlx::query(
            "UPDATE app_clients SET client_identifier = $1, description = $2, \
             updated_at = now() AT TIME ZONE 'utc' WHERE id = $3",
        )
        .bind(&identifier)
        .bind(&description)
        .bind(client.id)
        .execute(&mut *tx)
        .await;
        match updated {
            Ok(_) => {
                sync_access(&mut tx, client.id, &desired_access(&params)).await?;
                tx.commit().await?;
                ctx.flash("notice", "Client updated.");
                return Ok(ctx.redirect("/admin/app_clients"));
            }
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation()) =>
            {
                errors.push("Client identifier has already been taken".to_owned());
            }
            Err(e) => return Err(e.into()),
        }
    }
    ctx.flash_now("alert", errors.join(", "));
    let page = FormPage {
        chrome: ctx.chrome("app_clients"),
        id: Some(client.id),
        identifier,
        description: description.unwrap_or_default(),
        grid: grid_for(&state.db, Some(client.id)).await?,
    };
    Ok(ctx.page(StatusCode::UNPROCESSABLE_ENTITY, &page))
}

pub async fn destroy(
    State(state): State<AppState>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<String>,
) -> AdminResult {
    let client = find(&state.db, &id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM client_department_accesses WHERE app_client_id = $1")
        .bind(client.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM app_clients WHERE id = $1")
        .bind(client.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    ctx.flash("notice", "Client deleted.");
    Ok(ctx.redirect("/admin/app_clients"))
}
