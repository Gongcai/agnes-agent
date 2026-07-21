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
    Calendar,
    CalendarEvent,
    EventException,
    TaskList,
    Task,
    ReadingBook,
    ReadingHighlight,
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
            Self::Calendar => "calendar",
            Self::CalendarEvent => "calendar_event",
            Self::EventException => "event_exception",
            Self::TaskList => "task_list",
            Self::Task => "task",
            Self::ReadingBook => "reading_book",
            Self::ReadingHighlight => "reading_highlight",
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
            "seq",
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
        SyncEntityType::Calendar => &[
            "id",
            "name",
            "color",
            "timezone",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::CalendarEvent => &[
            "id",
            "calendar_id",
            "title",
            "description",
            "location",
            "starts_at",
            "ends_at",
            "timezone",
            "all_day",
            "recurrence_rule",
            "recurrence_id",
            "status",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::EventException => &[
            "id",
            "event_id",
            "original_occurrence",
            "replacement_event_id",
            "is_cancelled",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::TaskList => &[
            "id",
            "name",
            "color",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::Task => &[
            "id",
            "task_list_id",
            "parent_id",
            "title",
            "description",
            "status",
            "priority",
            "starts_at",
            "due_date",
            "due_at",
            "due_timezone",
            "is_important",
            "my_day_date",
            "completed_at",
            "recurrence_rule",
            "recurrence_anchor",
            "recurrence_source_id",
            "sort_order",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::ReadingBook => &[
            "id",
            "title",
            "author",
            "source_hash",
            "model_knows_content",
            "content_context_allowed",
            "content_context_decided",
            "progress_cfi",
            "created_at",
            "updated_at",
            "version",
            "deleted_at",
            "origin_device_id",
        ],
        SyncEntityType::ReadingHighlight => &[
            "id",
            "book_id",
            "cfi_range",
            "quote",
            "context_before",
            "context_after",
            "note",
            "color",
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
            "seq": 7,
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
                "seq": 7,
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
            SyncEntityType::Calendar,
            SyncEntityType::CalendarEvent,
            SyncEntityType::EventException,
            SyncEntityType::TaskList,
            SyncEntityType::Task,
            SyncEntityType::ReadingBook,
            SyncEntityType::ReadingHighlight,
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

    #[test]
    fn reading_book_projection_never_contains_device_local_bindings() {
        let source = json!({
            "id": "book-1",
            "collection_id": "local-collection",
            "document_id": "local-document",
            "local_path": "/home/user/book.epub",
            "title": "Portable book",
            "source_hash": "hash",
            "progress_cfi": "epubcfi(/6/2)",
            "version": 3
        });

        assert_eq!(
            project(SyncEntityType::ReadingBook, &source).unwrap(),
            json!({
                "id": "book-1",
                "title": "Portable book",
                "source_hash": "hash",
                "progress_cfi": "epubcfi(/6/2)",
                "version": 3
            })
        );
    }
}
