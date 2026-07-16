use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::protocol::{msg_type, Envelope};
use crate::agent::AgentManager;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

const EMBEDDING_BATCH_SIZE: usize = 64;

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
