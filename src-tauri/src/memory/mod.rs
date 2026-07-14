use crate::db::DbActorHandle;
use crate::error::AppResult;

/// V0.2 实现：sqlite-vec 向量 + FTS5 字符串混合检索（RRF 融合），限定 agent_id。
/// 当前返回空，仅占位以打通模块边界。
pub async fn search(
    _db: &DbActorHandle,
    _query: &str,
    _agent_id: &str,
) -> AppResult<Vec<String>> {
    Ok(vec![])
}
