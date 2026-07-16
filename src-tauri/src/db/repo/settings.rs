use rusqlite::Connection;

use crate::error::AppResult;

/// 获取指定键的设置值。
pub fn get(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        let val: String = row.get(0)?;
        Ok(Some(val))
    } else {
        Ok(None)
    }
}

/// 写入或更新指定键的设置值。
pub fn set(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

/// Delete a setting instead of retaining an empty sensitive row.
pub fn delete(conn: &Connection, key: &str) -> AppResult<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

pub fn list_with_prefix(conn: &Connection, prefix: &str) -> AppResult<Vec<(String, String)>> {
    let pattern = format!("{prefix}%");
    let mut stmt =
        conn.prepare("SELECT key, value FROM settings WHERE key LIKE ?1 ORDER BY key")?;
    let rows = stmt.query_map([pattern], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
