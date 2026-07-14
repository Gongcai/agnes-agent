pub mod actor;
pub mod migrations;
pub mod repo;
pub mod schema;

pub use actor::DbActorHandle;

use std::path::PathBuf;

/// 启动专用 DB 线程（rusqlite 单连接、串行写），返回句柄。
/// 所有 SQLite 写操作经由此单写者，保证写入顺序与事务边界清晰，且不阻塞 async runtime。
pub fn spawn_db_actor(db_path: PathBuf) -> DbActorHandle {
    actor::spawn(db_path)
}
