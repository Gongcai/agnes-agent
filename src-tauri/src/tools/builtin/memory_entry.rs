use serde_json::{json, Map, Value};

use crate::db::repo::memory::MemoryRow;
use crate::error::{AppError, AppResult};

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
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(AppError::Other(format!("Unsupported argument `{field}`")));
    }
    Ok(object)
}
