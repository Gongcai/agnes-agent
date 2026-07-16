use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};

const NON_SEMANTIC_FIELDS: &[&str] = &["updated_at", "version", "origin_device_id"];

#[derive(Debug, Clone, PartialEq)]
pub enum MergeOutcome {
    Merged(Value),
    Conflict(Vec<String>),
}

pub fn three_way_merge(
    base: Option<&Value>,
    local: &Value,
    remote: &Value,
) -> AppResult<MergeOutcome> {
    let local = local
        .as_object()
        .ok_or_else(|| AppError::Other("local conflict payload is not an object".into()))?;
    let remote = remote
        .as_object()
        .ok_or_else(|| AppError::Other("remote conflict payload is not an object".into()))?;
    let base = base.and_then(Value::as_object);

    let mut keys = BTreeSet::new();
    keys.extend(local.keys().cloned());
    keys.extend(remote.keys().cloned());
    if let Some(base) = base {
        keys.extend(base.keys().cloned());
    }

    let mut merged = Map::new();
    let mut conflicts = Vec::new();
    for key in keys {
        if NON_SEMANTIC_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if key == "created_at" {
            if let Some(value) = base
                .and_then(|object| object.get(&key))
                .or_else(|| min_json_value(local.get(&key), remote.get(&key)))
            {
                merged.insert(key, value.clone());
            }
            continue;
        }

        let base_value = base.and_then(|object| object.get(&key));
        let local_value = local.get(&key);
        let remote_value = remote.get(&key);
        let selected = if local_value == remote_value {
            local_value
        } else if base.is_some() && local_value == base_value {
            remote_value
        } else if base.is_some() && remote_value == base_value {
            local_value
        } else {
            conflicts.push(key);
            continue;
        };
        if let Some(value) = selected {
            merged.insert(key, value.clone());
        }
    }

    if conflicts.is_empty() {
        Ok(MergeOutcome::Merged(Value::Object(merged)))
    } else {
        Ok(MergeOutcome::Conflict(conflicts))
    }
}

fn min_json_value<'a>(left: Option<&'a Value>, right: Option<&'a Value>) -> Option<&'a Value> {
    match (left, right) {
        (Some(left), Some(right)) => {
            if left.to_string() <= right.to_string() {
                Some(left)
            } else {
                Some(right)
            }
        }
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn merges_non_overlapping_fields_and_ignores_revision_metadata() {
        let base = json!({
            "id": "agent-1",
            "name": "Base",
            "persona": "Base persona",
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "origin_device_id": "base"
        });
        let local = json!({
            "id": "agent-1",
            "name": "Local",
            "persona": "Base persona",
            "created_at": "1",
            "updated_at": "2",
            "version": 2,
            "origin_device_id": "local"
        });
        let remote = json!({
            "id": "agent-1",
            "name": "Base",
            "persona": "Remote persona",
            "created_at": "1",
            "updated_at": "3",
            "version": 2,
            "origin_device_id": "remote"
        });

        assert_eq!(
            three_way_merge(Some(&base), &local, &remote).unwrap(),
            MergeOutcome::Merged(json!({
                "id": "agent-1",
                "name": "Local",
                "persona": "Remote persona",
                "created_at": "1"
            }))
        );
    }

    #[test]
    fn preserves_same_field_edits_as_an_explicit_conflict() {
        let base = json!({"id": "memory-1", "content": "base"});
        let local = json!({"id": "memory-1", "content": "local"});
        let remote = json!({"id": "memory-1", "content": "remote"});

        assert_eq!(
            three_way_merge(Some(&base), &local, &remote).unwrap(),
            MergeOutcome::Conflict(vec!["content".into()])
        );
    }
}
