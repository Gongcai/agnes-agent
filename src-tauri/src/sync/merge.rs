use std::{cmp::Ordering, collections::BTreeSet};

use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};
use crate::sync::hlc::HybridTimestamp;

const NON_SEMANTIC_FIELDS: &[&str] = &["updated_at", "version", "origin_device_id"];
const SESSION_HLC_FIELDS: &[&str] = &[
    "title",
    "context_limit",
    "compress_threshold",
    "recency_window",
    "reserved_output_tokens",
    "model",
    "thinking_mode",
    "thinking_budget",
    "workspace_id",
    "pinned",
];

#[derive(Debug, Clone, PartialEq)]
pub enum MergeOutcome {
    Merged {
        payload: Value,
        resolved_fields: Vec<String>,
    },
    Conflict(Vec<String>),
}

pub fn three_way_merge(
    base: Option<&Value>,
    local: &Value,
    remote: &Value,
) -> AppResult<MergeOutcome> {
    merge_entity("", base, local, remote, "0-0-local", "0-0-remote")
}

pub fn merge_entity(
    entity_type: &str,
    base: Option<&Value>,
    local: &Value,
    remote: &Value,
    local_hlc: &str,
    remote_hlc: &str,
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
    let mut resolved_fields = Vec::new();
    let hlc_order = if entity_type == "session" {
        Some(compare_hlc(local_hlc, remote_hlc)?)
    } else {
        None
    };
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
            local_value.cloned()
        } else if base.is_some() && local_value == base_value {
            remote_value.cloned()
        } else if base.is_some() && remote_value == base_value {
            local_value.cloned()
        } else if entity_type == "session" && SESSION_HLC_FIELDS.contains(&key.as_str()) {
            resolved_fields.push(key.clone());
            select_by_order(
                local_value,
                remote_value,
                hlc_order.unwrap_or(Ordering::Equal),
            )
        } else if entity_type == "explicit_memory" && key == "content" {
            let Some(content) = merge_text(base_value, local_value, remote_value) else {
                conflicts.push(key);
                continue;
            };
            resolved_fields.push(key.clone());
            Some(content.into())
        } else {
            conflicts.push(key);
            continue;
        };
        if let Some(value) = selected {
            merged.insert(key, value);
        }
    }

    if conflicts.is_empty() {
        Ok(MergeOutcome::Merged {
            payload: Value::Object(merged),
            resolved_fields,
        })
    } else {
        Ok(MergeOutcome::Conflict(conflicts))
    }
}

fn compare_hlc(local: &str, remote: &str) -> AppResult<Ordering> {
    let local = HybridTimestamp::parse(local).map_err(AppError::Other)?;
    let remote = HybridTimestamp::parse(remote).map_err(AppError::Other)?;
    Ok((local.physical_ms, local.counter, local.node).cmp(&(
        remote.physical_ms,
        remote.counter,
        remote.node,
    )))
}

fn select_by_order(
    local: Option<&Value>,
    remote: Option<&Value>,
    order: Ordering,
) -> Option<Value> {
    match order {
        Ordering::Greater => local.cloned(),
        Ordering::Less => remote.cloned(),
        Ordering::Equal => max_json_value(local, remote).cloned(),
    }
}

fn merge_text(
    base: Option<&Value>,
    local: Option<&Value>,
    remote: Option<&Value>,
) -> Option<String> {
    diffy::merge(base?.as_str()?, local?.as_str()?, remote?.as_str()?).ok()
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

fn max_json_value<'a>(left: Option<&'a Value>, right: Option<&'a Value>) -> Option<&'a Value> {
    match (left, right) {
        (Some(left), Some(right)) => {
            if left.to_string() >= right.to_string() {
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
            MergeOutcome::Merged {
                payload: json!({
                    "id": "agent-1",
                    "name": "Local",
                    "persona": "Remote persona",
                    "created_at": "1"
                }),
                resolved_fields: Vec::new(),
            }
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

    #[test]
    fn session_same_field_uses_full_hlc_order_but_summary_stays_conflicted() {
        let base = json!({"id": "session-1", "title": "Base", "summary": "Base summary"});
        let local = json!({"id": "session-1", "title": "Local", "summary": "Base summary"});
        let remote = json!({"id": "session-1", "title": "Remote", "summary": "Base summary"});

        assert_eq!(
            merge_entity(
                "session",
                Some(&base),
                &local,
                &remote,
                "1000-0002-device-a",
                "1000-0001-device-z",
            )
            .unwrap(),
            MergeOutcome::Merged {
                payload: json!({
                    "id": "session-1",
                    "title": "Local",
                    "summary": "Base summary"
                }),
                resolved_fields: vec!["title".into()],
            }
        );
        assert_eq!(
            merge_entity(
                "session",
                Some(&base),
                &local,
                &remote,
                "1000-0002-device-a",
                "1000-0002-device-z",
            )
            .unwrap(),
            MergeOutcome::Merged {
                payload: json!({
                    "id": "session-1",
                    "title": "Remote",
                    "summary": "Base summary"
                }),
                resolved_fields: vec!["title".into()],
            }
        );

        let local = json!({"id": "session-1", "title": "Base", "summary": "Local"});
        let remote = json!({"id": "session-1", "title": "Base", "summary": "Remote"});
        assert_eq!(
            merge_entity(
                "session",
                Some(&base),
                &local,
                &remote,
                "1000-0002-device-a",
                "1000-0001-device-z",
            )
            .unwrap(),
            MergeOutcome::Conflict(vec!["summary".into()])
        );
    }

    #[test]
    fn explicit_memory_diff3_merges_separate_hunks_and_rejects_overlap() {
        let base_content = "# Profile\n\nLanguage: C++\n\n# Preferences\n\nTheme: light\n";
        let local_content =
            "# Profile\n\nLanguage: C++ and Rust\n\n# Preferences\n\nTheme: light\n";
        let remote_content = "# Profile\n\nLanguage: C++\n\n# Preferences\n\nTheme: dark\n";
        let payload = |content| json!({"id": "memory-1", "content": content});
        let outcome = merge_entity(
            "explicit_memory",
            Some(&payload(base_content)),
            &payload(local_content),
            &payload(remote_content),
            "1000-0001-local",
            "1000-0001-remote",
        )
        .unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                payload: payload(
                    "# Profile\n\nLanguage: C++ and Rust\n\n# Preferences\n\nTheme: dark\n"
                ),
                resolved_fields: vec!["content".into()],
            }
        );

        let local = payload("# Profile\n\nLanguage: Rust\n");
        let remote = payload("# Profile\n\nLanguage: Zig\n");
        assert_eq!(
            merge_entity(
                "explicit_memory",
                Some(&payload("# Profile\n\nLanguage: C++\n")),
                &local,
                &remote,
                "1000-0001-local",
                "1000-0001-remote",
            )
            .unwrap(),
            MergeOutcome::Conflict(vec!["content".into()])
        );
        assert_eq!(
            merge_entity(
                "explicit_memory",
                None,
                &local,
                &remote,
                "1000-0001-local",
                "1000-0001-remote",
            )
            .unwrap(),
            MergeOutcome::Conflict(vec!["content".into()])
        );
    }
}
