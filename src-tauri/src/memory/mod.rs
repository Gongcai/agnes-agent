use std::fs;
use std::path::PathBuf;

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

/// V0.2 实现：sqlite-vec 向量 + FTS5 字符串混合检索（RRF 融合），限定 agent_id。
pub async fn search(
    _db: &DbActorHandle,
    _query: &str,
    _agent_id: &str,
) -> AppResult<Vec<String>> {
    Ok(vec![])
}

/// 读取或初始化特定 Agent 的 USER.md 和 MEMORY.md 内存视图。
/// 路径位于：~/.agnes/agents/{agent_id}/memory/
pub fn load_explicit_memories(agent_id: &str) -> AppResult<(String, String)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Other("无法获取 HOME 环境变量，无法加载 explicit memories".into()))?;
        
    let mem_dir = home.join(".agnes").join("agents").join(agent_id).join("memory");
    if let Err(e) = fs::create_dir_all(&mem_dir) {
        return Err(AppError::Other(format!("创建记忆目录失败: {e}")));
    }
    
    let user_path = mem_dir.join("USER.md");
    if !user_path.exists() {
        let default_user = "# USER.md\n\n在此输入您的基础个人画像、偏好或背景信息，供助手每次对话时参考（AI 只读）。\n";
        let _ = fs::write(&user_path, default_user);
    }
    
    let memory_path = mem_dir.join("MEMORY.md");
    if !memory_path.exists() {
        let default_memory = "# MEMORY.md\n\n在此记录助手每次对话沉淀的事实与长期事实（AI 可写，用户可写）。\n";
        let _ = fs::write(&memory_path, default_memory);
    }
    
    let user_md = fs::read_to_string(&user_path)
        .map_err(|e| AppError::Other(format!("读取 USER.md 失败: {e}")))?;
    let memory_md = fs::read_to_string(&memory_path)
        .map_err(|e| AppError::Other(format!("读取 MEMORY.md 失败: {e}")))?;
        
    Ok((user_md, memory_md))
}

/// 将修改后的 USER.md 和 MEMORY.md 保存回特定 Agent 的磁盘路径。
pub fn save_explicit_memories(agent_id: &str, user_md: &str, memory_md: &str) -> AppResult<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Other("无法获取 HOME 环境变量，无法保存 explicit memories".into()))?;
        
    let mem_dir = home.join(".agnes").join("agents").join(agent_id).join("memory");
    fs::create_dir_all(&mem_dir)
        .map_err(|e| AppError::Other(format!("创建记忆目录失败: {e}")))?;
        
    fs::write(mem_dir.join("USER.md"), user_md)
        .map_err(|e| AppError::Other(format!("写入 USER.md 失败: {e}")))?;
    fs::write(mem_dir.join("MEMORY.md"), memory_md)
        .map_err(|e| AppError::Other(format!("写入 MEMORY.md 失败: {e}")))?;
        
    Ok(())
}
