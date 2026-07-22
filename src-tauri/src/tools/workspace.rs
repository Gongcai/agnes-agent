//! Workspace cwd resolution for Code and Home conversations.
use std::path::{Path, PathBuf};

use crate::db::DbActorHandle;

pub const HOME_WORKSPACE_DOCUMENTS_PATH: [&str; 2] = ["Agnes", "Home"];
pub const HOME_WORKSPACE_FALLBACK_PATH: [&str; 2] = ["workspaces", "home"];

fn append_segments(root: &Path, segments: &[&str]) -> PathBuf {
    segments
        .iter()
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

fn ensure_writable_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let probe = path.join(format!(".agnes-write-test-{}", uuid::Uuid::new_v4()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    drop(file);
    std::fs::remove_file(probe)
}

/// Create the shared Home workspace in the user-visible Documents directory.
/// Fall back to local app data when Documents is unavailable or protected.
pub fn prepare_home_workspace(
    document_dir: Option<&Path>,
    app_local_data_dir: &Path,
) -> std::io::Result<PathBuf> {
    if let Some(document_dir) = document_dir {
        let preferred = append_segments(document_dir, &HOME_WORKSPACE_DOCUMENTS_PATH);
        if ensure_writable_directory(&preferred).is_ok() {
            return Ok(preferred);
        }
    }

    let fallback = append_segments(app_local_data_dir, &HOME_WORKSPACE_FALLBACK_PATH);
    ensure_writable_directory(&fallback)?;
    Ok(fallback)
}

/// Resolve the effective cwd. Code sessions use their binding; sessions without a
/// logical workspace share the app-managed Home workspace on this device.
pub async fn resolve_workspace_cwd(
    db: &DbActorHandle,
    session_id: &str,
    home_workspace_dir: &Path,
) -> Option<PathBuf> {
    let session = db.get_session(session_id.to_string()).await.ok()??;
    let Some(ws_id) = session.workspace_id else {
        if std::fs::create_dir_all(home_workspace_dir).is_err() {
            return None;
        }
        return Some(home_workspace_dir.to_path_buf());
    };
    let ws = db.get_workspace(ws_id).await.ok()??;
    if ws.folder_path.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(ws.folder_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::agents::NewAgent;
    use crate::db::repo::sessions::NewSession;

    fn test_agent() -> NewAgent {
        NewAgent {
            id: "agent-1".into(),
            name: "Test Agent".into(),
            persona: String::new(),
            scenario: String::new(),
            system_prompt: String::new(),
            greeting: String::new(),
            example_dialogue: String::new(),
            model: String::new(),
            tool_policy: "{}".into(),
            avatar: String::new(),
            tags: String::new(),
            thinking_mode: "off".into(),
            thinking_budget: 0,
        }
    }

    fn test_session(id: &str, workspace_id: Option<&str>) -> NewSession {
        NewSession {
            id: id.into(),
            agent_id: "agent-1".into(),
            title: "Test".into(),
            context_limit: None,
            compress_threshold: None,
            recency_window: None,
            reserved_output_tokens: None,
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            permission_mode: "auto".into(),
            workspace_id: workspace_id.map(str::to_string),
            origin_device_id: None,
        }
    }

    #[tokio::test]
    async fn standalone_sessions_share_home_while_code_sessions_keep_their_binding() {
        let temp = tempfile::tempdir().unwrap();
        let db = crate::db::spawn_db_actor(temp.path().join("test.db"));
        db.insert_agent(test_agent()).await.unwrap();

        let home = prepare_home_workspace(
            Some(&temp.path().join("Documents")),
            &temp.path().join("app-local-data"),
        )
        .unwrap();
        db.insert_session(test_session("home-session", None))
            .await
            .unwrap();
        assert_eq!(
            resolve_workspace_cwd(&db, "home-session", &home).await,
            Some(home.clone())
        );
        assert!(home.is_dir());
        assert_eq!(
            home,
            temp.path().join("Documents").join("Agnes").join("Home")
        );

        let code = temp.path().join("code-project");
        std::fs::create_dir_all(&code).unwrap();
        db.insert_workspace(crate::db::repo::workspaces::NewWorkspace {
            id: "workspace-1".into(),
            agent_id: "agent-1".into(),
            name: "Code".into(),
            folder_path: code.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
        db.insert_session(test_session("code-session", Some("workspace-1")))
            .await
            .unwrap();
        assert_eq!(
            resolve_workspace_cwd(&db, "code-session", &home).await,
            Some(code)
        );
    }

    #[test]
    fn home_workspace_falls_back_when_documents_cannot_be_created() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_documents = temp.path().join("blocked-documents");
        std::fs::write(&blocked_documents, "not a directory").unwrap();
        let local_data = temp.path().join("app-local-data");

        let home = prepare_home_workspace(Some(&blocked_documents), &local_data).unwrap();

        assert_eq!(home, local_data.join("workspaces").join("home"));
        assert!(home.is_dir());
    }
}
