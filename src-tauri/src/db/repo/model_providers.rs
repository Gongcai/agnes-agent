use rusqlite::Connection;

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct ModelProviderRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub api_base: Option<String>,
    pub is_default: i32,
    pub models_json: Option<String>,
    pub extra_config: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewModelProvider {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub api_base: Option<String>,
    pub is_default: i32,
    pub models_json: Option<String>,
    pub extra_config: Option<String>,
}

pub fn list(conn: &Connection) -> AppResult<Vec<ModelProviderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, api_base, is_default, models_json, extra_config, \
         created_at, updated_at FROM model_providers ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ModelProviderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            api_base: r.get(3)?,
            is_default: r.get(4)?,
            models_json: r.get(5)?,
            extra_config: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Option<ModelProviderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, api_base, is_default, models_json, extra_config, \
         created_at, updated_at FROM model_providers WHERE id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    if let Some(r) = rows.next()? {
        Ok(Some(ModelProviderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            api_base: r.get(3)?,
            is_default: r.get(4)?,
            models_json: r.get(5)?,
            extra_config: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn upsert(conn: &Connection, row: &NewModelProvider) -> AppResult<String> {
    let now = super::agents::now();
    conn.execute(
        "INSERT INTO model_providers (id, name, kind, api_base, is_default, models_json, \
         extra_config, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(id) DO UPDATE SET \
         name = excluded.name, kind = excluded.kind, api_base = excluded.api_base, \
         is_default = excluded.is_default, models_json = excluded.models_json, \
         extra_config = excluded.extra_config, updated_at = excluded.updated_at",
        (
            &row.id,
            &row.name,
            &row.kind,
            &row.api_base,
            &row.is_default,
            &row.models_json,
            &row.extra_config,
            &now,
            &now,
        ),
    )?;
    Ok(row.id.clone())
}

pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM model_providers WHERE id = ?1", [id])?;
    Ok(())
}

pub fn get_default(conn: &Connection) -> AppResult<Option<ModelProviderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, api_base, is_default, models_json, extra_config, \
         created_at, updated_at FROM model_providers WHERE is_default = 1 LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(r) = rows.next()? {
        Ok(Some(ModelProviderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            api_base: r.get(3)?,
            is_default: r.get(4)?,
            models_json: r.get(5)?,
            extra_config: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn clear_default(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "UPDATE model_providers SET is_default = 0 WHERE is_default = 1",
        [],
    )?;
    Ok(())
}
