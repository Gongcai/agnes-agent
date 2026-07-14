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
        // 仅创建空文件；占位提示由前端展示，不写入真实内容以免被当作记忆发送给 AI
        let _ = fs::write(&user_path, "");
    }

    let memory_path = mem_dir.join("MEMORY.md");
    if !memory_path.exists() {
        let _ = fs::write(&memory_path, "");
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
