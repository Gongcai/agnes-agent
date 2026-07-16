use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEntityType {
    Agent,
    Session,
    Message,
    ExplicitMemory,
    Memory,
    Workspace,
}

impl SyncEntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Session => "session",
            Self::Message => "message",
            Self::ExplicitMemory => "explicit_memory",
            Self::Memory => "memory",
            Self::Workspace => "workspace",
        }
    }
}

pub fn project(entity_type: SyncEntityType, source: &Value) -> AppResult<Value> {
    let object = source
        .as_object()
        .ok_or_else(|| AppError::Other("Sync payload source must be an object".into()))?;
    let allowed = match entity_type {
        SyncEntityType::Agent => &[
            "id",
            "name",
            "persona",
            "scenario",
            "system_prompt",
            "greeting",
            "example_dialogue",
            "model",
            "tool_policy",
            "tags",
            "thinking_mode",
            "thinking_budget",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ][..],
        SyncEntityType::Session => &[
            "id",
            "agent_id",
            "title",
            "context_limit",
            "compress_threshold",
            "recency_window",
            "reserved_output_tokens",
            "model",
            "thinking_mode",
            "thinking_budget",
            "workspace_id",
            "summary",
            "summary_updated_at",
            "pinned",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::Message => &[
            "id",
            "session_id",
            "role",
            "parent_id",
            "selected_child_id",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::ExplicitMemory => &[
            "id",
            "agent_id",
            "kind",
            "content",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::Memory => &[
            "id",
            "agent_id",
            "name",
            "keywords",
            "content",
            "creator",
            "status",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::Workspace => &[
            "id",
            "agent_id",
            "name",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
    };

    let mut payload = Map::new();
    for field in allowed {
        if let Some(value) = object.get(*field) {
            payload.insert((*field).to_string(), value.clone());
        }
    }
    if entity_type == SyncEntityType::Message {
        payload.insert(
            "parts".into(),
            project_message_text_parts(object.get("parts")),
        );
    }
    Ok(Value::Object(payload))
}

fn project_message_text_parts(parts: Option<&Value>) -> Value {
    let projected = parts
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|part| part.get("kind").and_then(Value::as_str) == Some("text"))
        .map(|part| {
            let mut output = Map::new();
            for field in ["kind", "content", "ordinal"] {
                if let Some(value) = part.get(field) {
                    output.insert(field.to_string(), value.clone());
                }
            }
            Value::Object(output)
        })
        .collect::<Vec<_>>();
    Value::Array(projected)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn payload_projection_drops_secrets_paths_embeddings_and_private_parts() {
        let source = json!({
            "id": "message-1",
            "session_id": "session-1",
            "role": "assistant",
            "api_key": "must-not-sync",
            "folder_path": "/home/user/private",
            "cwd": "/home/user/project",
            "embedding_id": "local-vector",
            "parts": [
                {"kind": "text", "content": "Visible response", "ordinal": 0, "metadata": {"secret": true}},
                {"kind": "reasoning", "content": "Private chain of thought", "ordinal": 1},
                {"kind": "tool_result", "content": "raw stdout", "ordinal": 2}
            ]
        });

        let payload = project(SyncEntityType::Message, &source).unwrap();
        assert_eq!(
            payload,
            json!({
                "id": "message-1",
                "session_id": "session-1",
                "role": "assistant",
                "parts": [{"kind": "text", "content": "Visible response", "ordinal": 0}]
            })
        );
        let encoded = payload.to_string();
        for forbidden in [
            "must-not-sync",
            "/home/user/private",
            "local-vector",
            "Private chain of thought",
            "raw stdout",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn every_declared_entity_uses_an_explicit_allowlist() {
        let source = json!({
            "id": "entity-1",
            "content": "memory",
            "apiKey": "secret",
            "folder_path": "/tmp/private",
            "embedding_id": "embedding-1"
        });
        for entity_type in [
            SyncEntityType::Agent,
            SyncEntityType::Session,
            SyncEntityType::Message,
            SyncEntityType::ExplicitMemory,
            SyncEntityType::Memory,
            SyncEntityType::Workspace,
        ] {
            let encoded = project(entity_type, &source).unwrap().to_string();
            assert!(!encoded.contains("secret"));
            assert!(!encoded.contains("/tmp/private"));
            assert!(!encoded.contains("embedding-1"));
        }
    }

    #[test]
    fn session_projection_keeps_logical_workspace_but_drops_device_execution_policy() {
        let source = json!({
            "id": "session-1",
            "agent_id": "agent-1",
            "title": "Synced session",
            "pinned": 1,
            "permission_mode": "accept_edits",
            "workspace_id": "local-workspace",
            "parent_id": "not-a-session-field",
            "selected_child_id": "not-a-session-field"
        });

        assert_eq!(
            project(SyncEntityType::Session, &source).unwrap(),
            json!({
                "id": "session-1",
                "agent_id": "agent-1",
                "title": "Synced session",
                "pinned": 1,
                "workspace_id": "local-workspace"
            })
        );
    }

    #[test]
    fn workspace_projection_never_contains_the_local_binding() {
        let source = json!({
            "id": "workspace-1",
            "agent_id": "agent-1",
            "name": "Project",
            "folder_path": "/home/user/private-project",
            "last_validated_at": "123",
            "version": 2
        });

        assert_eq!(
            project(SyncEntityType::Workspace, &source).unwrap(),
            json!({
                "id": "workspace-1",
                "agent_id": "agent-1",
                "name": "Project",
                "version": 2
            })
        );
    }
}
