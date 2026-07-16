use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::protocol::{msg_type, Envelope};
use crate::agent::AgentManager;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

const EMBEDDING_BATCH_SIZE: usize = 64;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct MemoryIndexStatus {
    pub total: usize,
    pub indexed: usize,
    pub pending: usize,
    pub model_ref: Option<String>,
}

pub async fn agent_memory_index_status(
    db: &DbActorHandle,
    agent_id: &str,
    model_ref: Option<&str>,
) -> AppResult<MemoryIndexStatus> {
    let memories = db.list_memories(agent_id.to_string()).await?;
    Ok(summarize_memory_index(&memories, model_ref))
}

fn summarize_memory_index(
    memories: &[crate::db::repo::memory::MemoryRow],
    model_ref: Option<&str>,
) -> MemoryIndexStatus {
    let model_ref = model_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let indexed = model_ref.as_deref().map_or(0, |expected_model| {
        memories
            .iter()
            .filter(|memory| {
                let current_hash = crate::db::repo::memory::memory_content_hash(&memory.content);
                memory.embedding_id.is_some()
                    && memory.embedding_model.as_deref() == Some(expected_model)
                    && memory.embedding_content_hash.as_deref() == Some(current_hash.as_str())
            })
            .count()
    });
    MemoryIndexStatus {
        total: memories.len(),
        indexed,
        pending: memories.len().saturating_sub(indexed),
        model_ref,
    }
}

pub async fn ensure_agent_memory_index(
    db: &DbActorHandle,
    agent: &Arc<AgentManager>,
    agent_id: &str,
    config: &Value,
) -> AppResult<usize> {
    let model_ref = config
        .get("modelRef")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Other("Embedding model reference is missing".into()))?;
    let mut indexed = 0;
    loop {
        let memories = db.list_memories(agent_id.to_string()).await?;
        let stale = memories
            .into_iter()
            .filter(|memory| {
                let current_hash = crate::db::repo::memory::memory_content_hash(&memory.content);
                memory.embedding_id.is_none()
                    || memory.embedding_model.as_deref() != Some(model_ref)
                    || memory.embedding_content_hash.as_deref() != Some(current_hash.as_str())
            })
            .collect::<Vec<_>>();
        if stale.is_empty() {
            break;
        }

        for batch in stale.chunks(EMBEDDING_BATCH_SIZE) {
            let inputs = batch
                .iter()
                .map(|memory| memory.content.clone())
                .collect::<Vec<_>>();
            let vectors = request_embeddings(agent, config.clone(), inputs).await?;
            if vectors.len() != batch.len() {
                return Err(AppError::Other(
                    "Embedding sidecar returned an unexpected vector count".into(),
                ));
            }
            for (memory, vector) in batch.iter().zip(vectors) {
                if db
                    .upsert_memory_embedding(
                        uuid::Uuid::new_v4().to_string(),
                        memory.id.clone(),
                        model_ref.to_string(),
                        memory.content.clone(),
                        vector,
                    )
                    .await?
                {
                    indexed += 1;
                }
            }
        }
    }
    Ok(indexed)
}

async fn request_embeddings(
    agent: &Arc<AgentManager>,
    config: Value,
    inputs: Vec<String>,
) -> AppResult<Vec<Vec<f32>>> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    agent.register_embedding(request_id.clone(), tx);
    let envelope = Envelope {
        protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
        id: request_id.clone(),
        run_id: String::new(),
        session_id: String::new(),
        msg_type: msg_type::EMBEDDING_REQUEST.to_string(),
        created_at: String::new(),
        payload: json!({"config": config, "inputs": inputs}),
    };
    if let Err(error) = agent.send_to_agent(envelope) {
        agent.resolve_embedding(&request_id, json!({"error": error.to_string()}));
        return Err(error);
    }
    let payload = match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(payload)) => payload,
        Ok(Err(_)) => return Err(AppError::Other("Embedding response channel closed".into())),
        Err(_) => {
            agent.resolve_embedding(&request_id, json!({"error": "timeout"}));
            return Err(AppError::Other("Embedding request timed out".into()));
        }
    };
    if let Some(error) = payload.get("error").and_then(Value::as_str) {
        return Err(AppError::Other(error.to_string()));
    }
    let vectors = payload
        .get("vectors")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other("Embedding response has no vectors".into()))?;
    vectors
        .iter()
        .map(|vector| {
            let values = vector
                .as_array()
                .ok_or_else(|| AppError::Other("Embedding vector is not an array".into()))?;
            values
                .iter()
                .map(|value| {
                    value
                        .as_f64()
                        .map(|number| number as f32)
                        .filter(|number| number.is_finite())
                        .ok_or_else(|| AppError::Other("Embedding vector is invalid".into()))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::memory::MemoryRow;

    fn memory(
        id: &str,
        content: &str,
        embedding_model: Option<&str>,
        embedding_content_hash: Option<String>,
    ) -> MemoryRow {
        MemoryRow {
            id: id.into(),
            agent_id: "agent-a".into(),
            name: id.into(),
            keywords: Vec::new(),
            content: content.into(),
            creator: "user".into(),
            created_at: "1".into(),
            updated_at: "1".into(),
            status: "active".into(),
            version: 1,
            deleted_at: None,
            origin_device_id: None,
            embedding_id: embedding_model.map(|_| format!("embedding-{id}")),
            embedding_model: embedding_model.map(ToString::to_string),
            embedding_content_hash,
        }
    }

    #[test]
    fn status_only_counts_current_model_and_content_hash() {
        let current_content = "Current content";
        let memories = vec![
            memory(
                "current",
                current_content,
                Some("provider/qwen"),
                Some(crate::db::repo::memory::memory_content_hash(
                    current_content,
                )),
            ),
            memory(
                "old-model",
                "Other content",
                Some("provider/old"),
                Some(crate::db::repo::memory::memory_content_hash(
                    "Other content",
                )),
            ),
            memory(
                "old-content",
                "Changed content",
                Some("provider/qwen"),
                Some(crate::db::repo::memory::memory_content_hash(
                    "Previous content",
                )),
            ),
        ];

        assert_eq!(
            summarize_memory_index(&memories, Some("provider/qwen")),
            MemoryIndexStatus {
                total: 3,
                indexed: 1,
                pending: 2,
                model_ref: Some("provider/qwen".into()),
            }
        );
        assert_eq!(summarize_memory_index(&memories, None).indexed, 0);
    }
}
