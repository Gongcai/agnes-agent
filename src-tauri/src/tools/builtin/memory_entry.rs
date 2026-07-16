use serde_json::{json, Map, Value};

use crate::db::repo::memory::{MemoryRow, QueryEmbedding, MAX_EMBEDDING_DIMS};
use crate::error::{AppError, AppResult};
use crate::tools::builtin::ToolCtx;

const INTERNAL_EMBEDDING_KEY: &str = "__agnes_embedding";

pub(super) fn visible_memory(memory: &MemoryRow) -> Value {
    json!({
        "id": memory.id,
        "name": memory.name,
        "keywords": memory.keywords,
        "created_at": memory.created_at,
        "content": memory.content,
        "creator": memory.creator,
    })
}

pub(super) fn object_with_allowed_fields<'a>(
    args: &'a Value,
    allowed: &[&str],
) -> AppResult<&'a Map<String, Value>> {
    let object = args
        .as_object()
        .ok_or_else(|| AppError::Other("Tool arguments must be an object".into()))?;
    if let Some(field) = object.keys().find(|field| {
        field.as_str() != INTERNAL_EMBEDDING_KEY && !allowed.contains(&field.as_str())
    }) {
        return Err(AppError::Other(format!("Unsupported argument `{field}`")));
    }
    Ok(object)
}

pub(crate) fn trusted_embedding(args: &Value) -> Option<QueryEmbedding> {
    parse_embedding_value(args.get(INTERNAL_EMBEDDING_KEY)?)
}

pub(crate) fn parse_embedding_value(value: &Value) -> Option<QueryEmbedding> {
    let embedding = value.as_object()?;
    let model = embedding.get("model")?.as_str()?.trim();
    let values = embedding.get("vector")?.as_array()?;
    if model.is_empty() || values.is_empty() || values.len() > MAX_EMBEDDING_DIMS {
        return None;
    }
    let vector = values
        .iter()
        .map(|value| value.as_f64().map(|number| number as f32))
        .collect::<Option<Vec<_>>>()?;
    if vector.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(QueryEmbedding {
        model: model.to_string(),
        vector,
    })
}

pub(crate) fn public_arguments(args: &Value) -> Value {
    let mut sanitized = args.clone();
    if let Some(object) = sanitized.as_object_mut() {
        object.remove(INTERNAL_EMBEDDING_KEY);
    }
    sanitized
}

pub(super) async fn persist_trusted_embedding(ctx: &ToolCtx<'_>, memory_id: &str, content: &str) {
    let Some(embedding) = trusted_embedding(ctx.args) else {
        return;
    };
    if let Err(error) = ctx
        .db
        .upsert_memory_embedding(
            uuid::Uuid::new_v4().to_string(),
            memory_id.to_string(),
            embedding.model,
            content.to_string(),
            embedding.vector,
        )
        .await
    {
        eprintln!("[memory] Failed to index tool-written memory {memory_id}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_hides_trusted_embedding_arguments() {
        let args = json!({
            "query": "database",
            INTERNAL_EMBEDDING_KEY: {
                "model": "provider/embed-model",
                "vector": [1.0, 0.0, 0.0]
            }
        });
        let embedding = trusted_embedding(&args).unwrap();
        assert_eq!(embedding.model, "provider/embed-model");
        assert_eq!(embedding.vector, vec![1.0, 0.0, 0.0]);

        let public = public_arguments(&args);
        assert_eq!(public, json!({"query": "database"}));
    }
}
