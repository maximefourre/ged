//! `Department` tree and soft delete (port of `app/models/department.rb`).
//!
//! Every finder here is live-only (Rails' `default_scope`); the `*_with_deleted`
//! escape hatches and the unscoped reference checks are explicit.

use chrono::{NaiveDateTime, Utc};
use sqlx::{FromRow, PgPool};

/// How long a trashed document stays restorable (`Document::RETENTION`).
pub const DOCUMENT_RETENTION_DAYS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Department {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
}

#[derive(Debug)]
pub enum DepartmentError {
    /// `RecordNotDestroyed: Department is not a leaf`.
    NotALeaf,
    Db(sqlx::Error),
}

impl std::fmt::Display for DepartmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotALeaf => f.write_str("Department is not a leaf"),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for DepartmentError {}

impl From<sqlx::Error> for DepartmentError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

const COLUMNS: &str = "id, name, parent_id, created_at, updated_at, deleted_at";

/// UTC; Postgres keeps microseconds, as Rails' `timestamp(6)`.
fn now() -> NaiveDateTime {
    Utc::now().naive_utc()
}

impl Department {
    /// A live department.
    pub async fn find(db: &PgPool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM departments WHERE id = $1 AND deleted_at IS NULL"
        )))
        .bind(id)
        .fetch_optional(db)
        .await
    }

    /// Live or trashed (`Department.with_deleted`).
    pub async fn find_with_deleted(db: &PgPool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM departments WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(db)
        .await
    }

    /// This department first, then each live parent up to the root. The walk
    /// stops at a trashed ancestor and never revisits a node (a corrupt cycle
    /// ends instead of looping). Iterative, so a deep tree costs no stack.
    pub async fn self_and_ancestors(&self, db: &PgPool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "WITH RECURSIVE chain AS (
                SELECT d.id, d.name, d.parent_id, d.created_at, d.updated_at, d.deleted_at,
                       0 AS depth, ARRAY[d.id] AS path
                FROM departments d WHERE d.id = $1 AND d.deleted_at IS NULL
                UNION ALL
                SELECT p.id, p.name, p.parent_id, p.created_at, p.updated_at, p.deleted_at,
                       c.depth + 1, c.path || p.id
                FROM departments p JOIN chain c ON p.id = c.parent_id
                WHERE p.deleted_at IS NULL AND NOT p.id = ANY(c.path)
            )
            SELECT id, name, parent_id, created_at, updated_at, deleted_at
            FROM chain ORDER BY depth",
        )
        .bind(self.id)
        .fetch_all(db)
        .await
    }

    /// This department and every live descendant.
    pub async fn subtree_ids(&self, db: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "WITH RECURSIVE subtree(id) AS (
                SELECT $1::bigint
                UNION
                SELECT d.id FROM departments d JOIN subtree s ON d.parent_id = s.id
                WHERE d.deleted_at IS NULL
            ) SELECT id FROM subtree ORDER BY id",
        )
        .bind(self.id)
        .fetch_all(db)
        .await
    }

    /// No children at all — trashed ones count (unscoped).
    pub async fn is_leaf(&self, db: &PgPool) -> Result<bool, sqlx::Error> {
        let has_child: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)")
                .bind(self.id)
                .fetch_one(db)
                .await?;
        Ok(!has_child)
    }

    /// Hides the department and trashes its live documents with the same
    /// `deleted_at` and `purge_at` (`+ DOCUMENT_RETENTION_DAYS`), atomically.
    /// Leaf-only. The row is locked first, so a child being created at the
    /// same time either commits before the leaf check or waits for this
    /// transaction (a foreign key insert needs a share lock on its parent).
    pub async fn soft_delete(&self, db: &PgPool) -> Result<(), DepartmentError> {
        let now = now();
        let purge_at = now + chrono::Duration::days(DOCUMENT_RETENTION_DAYS);
        let mut tx = db.begin().await?;
        sqlx::query("SELECT id FROM departments WHERE id = $1 FOR UPDATE")
            .bind(self.id)
            .fetch_optional(&mut *tx)
            .await?;
        let has_child: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)")
                .bind(self.id)
                .fetch_one(&mut *tx)
                .await?;
        if has_child {
            return Err(DepartmentError::NotALeaf);
        }
        sqlx::query("UPDATE departments SET deleted_at = $1, updated_at = $1 WHERE id = $2")
            .bind(now)
            .bind(self.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE documents SET deleted_at = $1, purge_at = $2, updated_at = $1 \
             WHERE department_id = $3 AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(purge_at)
        .bind(self.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Un-hides the department and restores all of its trashed documents.
    pub async fn restore(&self, db: &PgPool) -> Result<(), sqlx::Error> {
        let now = now();
        let mut tx = db.begin().await?;
        sqlx::query("UPDATE departments SET deleted_at = NULL, updated_at = $1 WHERE id = $2")
            .bind(now)
            .bind(self.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE documents SET deleted_at = NULL, purge_at = NULL, updated_at = $1 \
             WHERE department_id = $2 AND deleted_at IS NOT NULL",
        )
        .bind(now)
        .bind(self.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// Un-hides the department only (the single-document restore path).
    pub async fn clear_soft_delete(&self, db: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE departments SET deleted_at = NULL, updated_at = $1 WHERE id = $2")
            .bind(now())
            .bind(self.id)
            .execute(db)
            .await?;
        Ok(())
    }

    /// Whether nothing references this department any more — documents
    /// (trashed included), upload requests, children (trashed included).
    /// Checked unscoped on purpose: the purge of a trashed department waits
    /// on rows a default-scoped association would never show.
    pub async fn references_cleared(&self, db: &PgPool) -> Result<bool, sqlx::Error> {
        let referenced: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM documents WHERE department_id = $1) \
                 OR EXISTS (SELECT 1 FROM upload_requests WHERE department_id = $1) \
                 OR EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)",
        )
        .bind(self.id)
        .fetch_one(db)
        .await?;
        Ok(!referenced)
    }

    /// Hard delete, with the department's grants (`dependent: :destroy`).
    pub async fn purge(&self, db: &PgPool) -> Result<(), sqlx::Error> {
        let mut tx = db.begin().await?;
        sqlx::query("DELETE FROM client_department_accesses WHERE department_id = $1")
            .bind(self.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM departments WHERE id = $1")
            .bind(self.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Trashed departments (`Department.deleted`).
    pub async fn deleted(db: &PgPool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM departments WHERE deleted_at IS NOT NULL ORDER BY id"
        )))
        .fetch_all(db)
        .await
    }
}
