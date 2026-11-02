//! `AppClient` and its department grants (port of `app/models/app_client.rb`).

use sqlx::{FromRow, PgPool};

/// Read and write are two independent permissions. A closed type: a typo'd or
/// unknown mode cannot be expressed, so it can never fall through to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    Read,
    Write,
}

impl AccessMode {
    /// The word used in the 403 message (`no read access to this department`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct AppClient {
    pub id: i64,
    pub client_identifier: String,
    pub description: Option<String>,
}

// Both walks only ever cross *live* departments, like Rails' default scope:
// a trashed department ends the chain upward (no grant above it applies) and
// is skipped downward. `UNION` (not `UNION ALL`) makes a corrupt parent cycle
// terminate; the recursion is iterative in Postgres, so depth costs no stack.
const CAN_READ: &str = "
WITH RECURSIVE chain(id, parent_id) AS (
    SELECT id, parent_id FROM departments WHERE id = $1 AND deleted_at IS NULL
    UNION
    SELECT d.id, d.parent_id FROM departments d
    JOIN chain c ON d.id = c.parent_id WHERE d.deleted_at IS NULL
)
SELECT EXISTS (
    SELECT 1 FROM chain JOIN client_department_accesses a ON a.department_id = chain.id
    WHERE a.app_client_id = $2 AND a.can_read
)";

const CAN_WRITE: &str = "
WITH RECURSIVE chain(id, parent_id) AS (
    SELECT id, parent_id FROM departments WHERE id = $1 AND deleted_at IS NULL
    UNION
    SELECT d.id, d.parent_id FROM departments d
    JOIN chain c ON d.id = c.parent_id WHERE d.deleted_at IS NULL
)
SELECT EXISTS (
    SELECT 1 FROM chain JOIN client_department_accesses a ON a.department_id = chain.id
    WHERE a.app_client_id = $2 AND a.can_write
)";

const IDS_READ: &str = "
WITH RECURSIVE subtree(id) AS (
    SELECT d.id FROM departments d
    JOIN client_department_accesses a ON a.department_id = d.id
    WHERE a.app_client_id = $1 AND a.can_read AND d.deleted_at IS NULL
    UNION
    SELECT d.id FROM departments d JOIN subtree s ON d.parent_id = s.id
    WHERE d.deleted_at IS NULL
)
SELECT id FROM subtree ORDER BY id";

const IDS_WRITE: &str = "
WITH RECURSIVE subtree(id) AS (
    SELECT d.id FROM departments d
    JOIN client_department_accesses a ON a.department_id = d.id
    WHERE a.app_client_id = $1 AND a.can_write AND d.deleted_at IS NULL
    UNION
    SELECT d.id FROM departments d JOIN subtree s ON d.parent_id = s.id
    WHERE d.deleted_at IS NULL
)
SELECT id FROM subtree ORDER BY id";

impl AppClient {
    pub async fn find_by_identifier(
        db: &PgPool,
        client_identifier: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, client_identifier, description FROM app_clients \
             WHERE client_identifier = $1",
        )
        .bind(client_identifier)
        .fetch_optional(db)
        .await
    }

    /// Whether this client may act on `department_id` in `mode`: a grant on
    /// the department or on any live ancestor, with that mode's flag set.
    pub async fn can_access(
        &self,
        db: &PgPool,
        department_id: i64,
        mode: AccessMode,
    ) -> Result<bool, sqlx::Error> {
        let sql = match mode {
            AccessMode::Read => CAN_READ,
            AccessMode::Write => CAN_WRITE,
        };
        sqlx::query_scalar(sql)
            .bind(department_id)
            .bind(self.id)
            .fetch_one(db)
            .await
    }

    /// Every live department in a subtree granted for `mode`, sorted, once.
    pub async fn accessible_department_ids(
        &self,
        db: &PgPool,
        mode: AccessMode,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let sql = match mode {
            AccessMode::Read => IDS_READ,
            AccessMode::Write => IDS_WRITE,
        };
        sqlx::query_scalar(sql).bind(self.id).fetch_all(db).await
    }
}
