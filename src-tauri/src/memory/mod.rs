use serde_json::{json, Value};

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

pub async fn load_filesystem_snapshot(db: &DbActorHandle, agent_id: &str) -> AppResult<Value> {
    let snapshot = db.load_memory_prompt_snapshot(agent_id.to_string()).await?;

    Ok(json!({
        "profile": snapshot.profile,
        "preferences": snapshot.preferences,
        "files": snapshot.listing.files,
        "listingEpoch": snapshot.listing.listing_epoch,
    }))
}

pub async fn agent_id_for_session(db: &DbActorHandle, session_id: &str) -> AppResult<String> {
    db.get_session(session_id.to_string())
        .await?
        .map(|session| session.agent_id)
        .ok_or_else(|| AppError::Other("Session was not found for memory access".into()))
}
