//! workspace cwd 解析：根据 session.workspace_id 取对应工作区的 folder_path，
//! 作为工具执行的默认工作目录。
use std::path::PathBuf;

use crate::db::DbActorHandle;

/// 解析某会话所属工作区的文件夹路径。无工作区则返回 None。
pub async fn resolve_workspace_cwd(db: &DbActorHandle, session_id: &str) -> Option<PathBuf> {
    let session = db.get_session(session_id.to_string()).await.ok()??;
    let ws_id = session.workspace_id?;
    let ws = db.get_workspace(ws_id).await.ok()??;
    Some(PathBuf::from(ws.folder_path))
}
