use serde::Serialize;
use serde_json::{json, Value};

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUserProfileSnapshot {
    pub content: String,
    pub mode: crate::user_profile::UserProfileInheritanceMode,
    pub effective: bool,
    pub user_md_empty: bool,
    pub version: Option<String>,
}

fn resolve_user_profile(
    snapshot: &crate::db::repo::memory_files::MemoryPromptSnapshot,
) -> AppResult<(AgentUserProfileSnapshot, String)> {
    let profile = match snapshot.base_profile_json.as_deref() {
        Some(raw) => serde_json::from_str::<crate::user_profile::UserProfile>(raw)
            .map_err(|error| AppError::Other(format!("用户资料无法解析：{error}")))?,
        None => crate::user_profile::UserProfile::default(),
    };
    let user_md = &snapshot.user_md.content;
    let effective = crate::user_profile::should_inherit(snapshot.user_md.mode, user_md);
    let merged =
        crate::user_profile::merge_user_profile(user_md, &profile.to_markdown(), effective);
    Ok((
        AgentUserProfileSnapshot {
            content: user_md.clone(),
            mode: snapshot.user_md.mode,
            effective,
            user_md_empty: user_md.trim().is_empty(),
            version: snapshot.user_md.version.clone(),
        },
        merged,
    ))
}

pub async fn load_filesystem_snapshot(db: &DbActorHandle, agent_id: &str) -> AppResult<Value> {
    let snapshot = db.load_memory_prompt_snapshot(agent_id.to_string()).await?;
    let (user_profile_snapshot, user_profile) = resolve_user_profile(&snapshot)?;

    Ok(json!({
        "user": user_profile,
        "userMode": user_profile_snapshot.mode,
        "profile": snapshot.profile,
        "preferences": snapshot.preferences,
        "files": snapshot.listing.files,
        "listingEpoch": snapshot.listing.listing_epoch,
    }))
}

pub async fn load_agent_user_profile(
    db: &DbActorHandle,
    agent_id: &str,
) -> AppResult<AgentUserProfileSnapshot> {
    let snapshot = db.load_memory_prompt_snapshot(agent_id.to_string()).await?;
    Ok(resolve_user_profile(&snapshot)?.0)
}

pub async fn agent_id_for_session(db: &DbActorHandle, session_id: &str) -> AppResult<String> {
    db.get_session(session_id.to_string())
        .await?
        .map(|session| session.agent_id)
        .ok_or_else(|| AppError::Other("Session was not found for memory access".into()))
}
