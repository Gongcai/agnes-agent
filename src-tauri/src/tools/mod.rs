pub mod builtin;
pub mod executor;
pub mod permissions;
pub mod policy;
pub mod review;
pub mod sandbox;
pub mod terminal;
pub mod workspace;

pub use executor::ToolExecutor;
pub use permissions::PermissionMode;
pub use policy::ToolPolicy;

pub(crate) fn audit_arguments(tool: &str, arguments: &serde_json::Value) -> serde_json::Value {
    let mut sanitized = arguments.clone();
    if matches!(
        tool,
        "memory_write" | "memory_str_replace" | "memory_append"
    ) {
        if let Some(object) = sanitized.as_object_mut() {
            for field in ["content", "old_str", "new_str"] {
                if let Some(value) = object.get_mut(field) {
                    let bytes = value.as_str().map(str::len).unwrap_or_default();
                    *value = serde_json::Value::String(format!("[redacted:{bytes} bytes]"));
                }
            }
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn memory_audit_arguments_do_not_copy_file_contents() {
        let sanitized = super::audit_arguments(
            "memory_str_replace",
            &json!({
                "path": "/topics/private.md",
                "old_str": "private old fact",
                "new_str": "private new fact",
                "if_version": "v1:abc"
            }),
        );
        let encoded = sanitized.to_string();
        assert!(encoded.contains("/topics/private.md"));
        assert!(!encoded.contains("private old fact"));
        assert!(!encoded.contains("private new fact"));
    }
}
