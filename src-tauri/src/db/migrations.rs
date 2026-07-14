use rusqlite::Connection;

use crate::error::AppResult;

/// 应用 DB 建表。当前为幂等 CREATE TABLE IF NOT EXISTS；
/// 后续结构变更在此追加新迁移（或引入 refinery），保持向前兼容。
pub fn apply(conn: &mut Connection) -> AppResult<()> {
    conn.execute_batch(crate::db::schema::SCHEMA)?;
    Ok(())
}
