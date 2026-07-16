use std::fs;
use std::path::PathBuf;

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

fn normalize_memory_text(text: String) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed == "# USER.md"
        || trimmed == "# MEMORY.md"
        || trimmed.contains("在此输入您的基础个人画像")
        || trimmed.contains("在此记录助手每次对话沉淀的事实")
    {
        return String::new();
    }
    text
}

fn memory_dir(agent_id: &str) -> AppResult<PathBuf> {
    if agent_id.is_empty()
        || !agent_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(AppError::Other(
            "Invalid agent id for memory storage".into(),
        ));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Other("HOME is unavailable for memory storage".into()))?;
    Ok(home
        .join(".agnes")
        .join("agents")
        .join(agent_id)
        .join("memory"))
}

fn load_memory_views(agent_id: &str) -> AppResult<(String, String)> {
    let directory = memory_dir(agent_id)?;
    fs::create_dir_all(&directory)?;
    let user_path = directory.join("USER.md");
    let memory_path = directory.join("MEMORY.md");
    if !user_path.exists() {
        fs::write(&user_path, "")?;
    }
    if !memory_path.exists() {
        fs::write(&memory_path, "")?;
    }
    Ok((
        normalize_memory_text(fs::read_to_string(user_path)?),
        normalize_memory_text(fs::read_to_string(memory_path)?),
    ))
}

fn save_memory_views(agent_id: &str, user_md: &str, memory_md: &str) -> AppResult<()> {
    let directory = memory_dir(agent_id)?;
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("USER.md"), user_md)?;
    fs::write(directory.join("MEMORY.md"), memory_md)?;
    Ok(())
}

/// Load explicit memories from SQLite and refresh the Markdown materialized views.
pub async fn load_explicit_memories(
    db: &DbActorHandle,
    agent_id: &str,
) -> AppResult<(String, String)> {
    let rows = db.list_explicit_memories(agent_id.to_string()).await?;
    let user_row = rows
        .iter()
        .find(|row| row.kind == crate::db::repo::explicit_memories::USER_MD_KIND);
    let memory_row = rows
        .iter()
        .find(|row| row.kind == crate::db::repo::explicit_memories::MEMORY_MD_KIND);
    let db_user = user_row.map(|row| {
        if row.deleted_at.is_none() {
            row.content.clone()
        } else {
            String::new()
        }
    });
    let db_memory = memory_row.map(|row| {
        if row.deleted_at.is_none() {
            row.content.clone()
        } else {
            String::new()
        }
    });
    let (view_user, view_memory) = load_memory_views(agent_id)?;

    let user_md = normalize_memory_text(db_user.clone().unwrap_or(view_user));
    let memory_md = normalize_memory_text(db_memory.clone().unwrap_or(view_memory));
    if user_row.is_none() || memory_row.is_none() {
        db.save_explicit_memories(
            agent_id.to_string(),
            user_md.clone(),
            memory_md.clone(),
        )
        .await?;
    }
    save_memory_views(agent_id, &user_md, &memory_md)?;
    Ok((user_md, memory_md))
}

/// Save both explicit memory documents through the SQLite canonical store.
pub async fn save_explicit_memories(
    db: &DbActorHandle,
    agent_id: &str,
    user_md: &str,
    memory_md: &str,
) -> AppResult<()> {
    let user_md = normalize_memory_text(user_md.to_string());
    let memory_md = normalize_memory_text(memory_md.to_string());
    db.save_explicit_memories(agent_id.to_string(), user_md.clone(), memory_md.clone())
        .await?;
    save_memory_views(agent_id, &user_md, &memory_md)
}

pub async fn agent_id_for_session(db: &DbActorHandle, session_id: &str) -> AppResult<String> {
    db.get_session(session_id.to_string())
        .await?
        .map(|session| session.agent_id)
        .ok_or_else(|| AppError::Other("Session was not found for memory access".into()))
}

pub async fn load_memory_md_for_session(
    db: &DbActorHandle,
    session_id: &str,
) -> AppResult<(String, String)> {
    let agent_id = agent_id_for_session(db, session_id).await?;
    let (_, memory_md) = load_explicit_memories(db, &agent_id).await?;
    Ok((agent_id, memory_md))
}

pub async fn save_memory_md(db: &DbActorHandle, agent_id: &str, memory_md: &str) -> AppResult<()> {
    let (user_md, _) = load_explicit_memories(db, agent_id).await?;
    save_explicit_memories(db, agent_id, &user_md, memory_md).await
}
