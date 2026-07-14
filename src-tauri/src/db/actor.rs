use std::path::PathBuf;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::db::migrations;
use crate::db::repo;
use crate::error::{AppError, AppResult};

/// DB 线程处理的命令。完整涵盖会话、消息和工具审计。
pub enum DbCommand {
    ListAgents {
        resp: oneshot::Sender<AppResult<Vec<repo::agents::AgentRow>>>,
    },
    InsertAgent {
        row: repo::agents::NewAgent,
        resp: oneshot::Sender<AppResult<String>>,
    },
    UpdateAgentModel {
        agent_id: String,
        model: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateAgent {
        id: String,
        changes: repo::agents::AgentUpdate,
        resp: oneshot::Sender<AppResult<()>>,
    },
    DeleteAgent {
        id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    InsertMemory {
        row: repo::memory::NewMemory,
        resp: oneshot::Sender<AppResult<()>>,
    },
    GetSetting {
        key: String,
        resp: oneshot::Sender<AppResult<Option<String>>>,
    },
    SetSetting {
        key: String,
        value: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    InsertEmbedding {
        embedding_id: String,
        ref_type: String,
        ref_id: String,
        model: String,
        dims: i32,
        content_hash: String,
        vector: Vec<f32>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    SearchMemories {
        query_text: String,
        query_vector: Option<Vec<f32>>,
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<String>>>,
    },
    ListSessions {
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::sessions::SessionRow>>>,
    },
    GetSession {
        id: String,
        resp: oneshot::Sender<AppResult<Option<repo::sessions::SessionRow>>>,
    },
    InsertSession {
        row: repo::sessions::NewSession,
        resp: oneshot::Sender<AppResult<String>>,
    },
    UpdateSessionTitle {
        id: String,
        title: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateSessionSummary {
        id: String,
        summary: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    DeleteSession {
        id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    SetSessionPin {
        id: String,
        pinned: bool,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateSessionLlm {
        id: String,
        model: String,
        thinking_mode: String,
        thinking_budget: i64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListMessagesWithParts {
        session_id: String,
        resp: oneshot::Sender<AppResult<Vec<(repo::messages::MessageRow, Vec<repo::messages::MessagePartRow>)>>>,
    },
    InsertMessage {
        msg: repo::messages::NewMessage,
        parts: Vec<repo::messages::NewMessagePart>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    InsertMessageParts {
        parts: Vec<repo::messages::NewMessagePart>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateMessageStatus {
        id: String,
        status: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    InsertToolCall {
        row: repo::tools::NewToolCall,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateToolCallRunning {
        id: String,
        cwd: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    UpdateToolCallComplete {
        id: String,
        status: String,
        result: Option<String>,
        exit_code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    GetToolCall {
        id: String,
        resp: oneshot::Sender<AppResult<Option<repo::tools::ToolCallRow>>>,
    },
    ListToolCalls {
        session_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::tools::ToolCallRow>>>,
    },
    ListModelProviders {
        resp: oneshot::Sender<AppResult<Vec<repo::model_providers::ModelProviderRow>>>,
    },
    GetModelProvider {
        id: String,
        resp: oneshot::Sender<AppResult<Option<repo::model_providers::ModelProviderRow>>>,
    },
    UpsertModelProvider {
        row: repo::model_providers::NewModelProvider,
        set_default: bool,
        resp: oneshot::Sender<AppResult<String>>,
    },
    DeleteModelProvider {
        id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    GetDefaultModelProvider {
        resp: oneshot::Sender<AppResult<Option<repo::model_providers::ModelProviderRow>>>,
    },
}

/// 对外句柄：命令通过 mpsc 发给 DB 线程，结果经 oneshot 回传。
#[derive(Clone)]
pub struct DbActorHandle {
    tx: mpsc::UnboundedSender<DbCommand>,
}

impl DbActorHandle {
    fn send(&self, cmd: DbCommand) -> AppResult<()> {
        self.tx
            .send(cmd)
            .map_err(|_| AppError::Other("db actor 已关闭".into()))
    }

    pub async fn list_agents(&self) -> AppResult<Vec<repo::agents::AgentRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListAgents { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_agent(&self, row: repo::agents::NewAgent) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertAgent { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_agent_model(&self, agent_id: String, model: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateAgentModel { agent_id, model, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_agent(
        &self,
        id: String,
        changes: repo::agents::AgentUpdate,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateAgent { id, changes, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_agent(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteAgent { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_memory(&self, row: repo::memory::NewMemory) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertMemory { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_setting(&self, key: String) -> AppResult<Option<String>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetSetting { key, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn set_setting(&self, key: String, value: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SetSetting { key, value, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_embedding(
        &self,
        embedding_id: String,
        ref_type: String,
        ref_id: String,
        model: String,
        dims: i32,
        content_hash: String,
        vector: Vec<f32>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertEmbedding {
            embedding_id,
            ref_type,
            ref_id,
            model,
            dims,
            content_hash,
            vector,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn search_memories(
        &self,
        query_text: String,
        query_vector: Option<Vec<f32>>,
        agent_id: String,
    ) -> AppResult<Vec<String>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SearchMemories {
            query_text,
            query_vector,
            agent_id,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_sessions(&self, agent_id: String) -> AppResult<Vec<repo::sessions::SessionRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListSessions { agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_session(&self, id: String) -> AppResult<Option<repo::sessions::SessionRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetSession { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_session(&self, row: repo::sessions::NewSession) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertSession { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_session_title(&self, id: String, title: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateSessionTitle { id, title, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_session_summary(&self, id: String, summary: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateSessionSummary { id, summary, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_session(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteSession { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn set_session_pin(&self, id: String, pinned: bool) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SetSessionPin { id, pinned, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_session_llm(
        &self,
        id: String,
        model: String,
        thinking_mode: String,
        thinking_budget: i64,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateSessionLlm {
            id,
            model,
            thinking_mode,
            thinking_budget,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_messages_with_parts(
        &self,
        session_id: String,
    ) -> AppResult<Vec<(repo::messages::MessageRow, Vec<repo::messages::MessagePartRow>)>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListMessagesWithParts { session_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_message(
        &self,
        msg: repo::messages::NewMessage,
        parts: Vec<repo::messages::NewMessagePart>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertMessage { msg, parts, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_message_parts(
        &self,
        parts: Vec<repo::messages::NewMessagePart>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertMessageParts { parts, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_message_status(&self, id: String, status: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateMessageStatus { id, status, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_tool_call(&self, row: repo::tools::NewToolCall) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertToolCall { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_tool_call_running(&self, id: String, cwd: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateToolCallRunning { id, cwd, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_tool_call_complete(
        &self,
        id: String,
        status: String,
        result: Option<String>,
        exit_code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateToolCallComplete {
            id,
            status,
            result,
            exit_code,
            stdout,
            stderr,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_tool_call(&self, id: String) -> AppResult<Option<repo::tools::ToolCallRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetToolCall { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_tool_calls(&self, session_id: String) -> AppResult<Vec<repo::tools::ToolCallRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListToolCalls { session_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_model_providers(&self) -> AppResult<Vec<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListModelProviders { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_model_provider(&self, id: String) -> AppResult<Option<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetModelProvider { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn upsert_model_provider(&self, row: repo::model_providers::NewModelProvider, set_default: bool) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpsertModelProvider { row, set_default, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_model_provider(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteModelProvider { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_default_model_provider(&self) -> AppResult<Option<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetDefaultModelProvider { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
}

/// 在独立 OS 线程跑 rusqlite 单连接；该线程用 `blocking_recv` 消费命令。
pub fn spawn(db_path: PathBuf) -> DbActorHandle {
    unsafe {
        let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<DbCommand>();
    std::thread::spawn(move || {
        let mut conn = match Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[db] 打开失败：{e}");
                return;
            }
        };

        // 启用外键约束
        if let Err(e) = conn.execute("PRAGMA foreign_keys = ON", []) {
            eprintln!("[db] 启用外键约束失败：{e}");
        }

        if let Err(e) = migrations::apply(&mut conn) {
            eprintln!("[db] 迁移失败：{e}");
            return;
        }

        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                DbCommand::ListAgents { resp } => {
                    let _ = resp.send(repo::agents::list(&conn));
                }
                DbCommand::InsertAgent { row, resp } => {
                    let _ = resp.send(repo::agents::insert(&conn, &row));
                }
                DbCommand::UpdateAgentModel { agent_id, model, resp } => {
                    let _ = resp.send(repo::agents::update_model(&conn, &agent_id, &model));
                }
                DbCommand::UpdateAgent { id, changes, resp } => {
                    let _ = resp.send(repo::agents::update(&conn, &id, &changes));
                }
                DbCommand::DeleteAgent { id, resp } => {
                    let _ = resp.send(repo::agents::delete(&conn, &id));
                }
                DbCommand::InsertMemory { row, resp } => {
                    let _ = resp.send(repo::memory::insert(&conn, &row));
                }
                DbCommand::GetSetting { key, resp } => {
                    let _ = resp.send(repo::settings::get(&conn, &key));
                }
                DbCommand::SetSetting { key, value, resp } => {
                    let _ = resp.send(repo::settings::set(&conn, &key, &value));
                }
                DbCommand::InsertEmbedding {
                    embedding_id,
                    ref_type,
                    ref_id,
                    model,
                    dims,
                    content_hash,
                    vector,
                    resp,
                } => {
                    let _ = resp.send(repo::memory::insert_embedding(
                        &conn,
                        &embedding_id,
                        &ref_type,
                        &ref_id,
                        &model,
                        dims,
                        &content_hash,
                        &vector,
                    ));
                }
                DbCommand::SearchMemories {
                    query_text,
                    query_vector,
                    agent_id,
                    resp,
                } => {
                    let vec_ref = query_vector.as_deref();
                    let _ = resp.send(repo::memory::search(&conn, &query_text, vec_ref, &agent_id));
                }
                DbCommand::ListSessions { agent_id, resp } => {
                    let _ = resp.send(repo::sessions::list(&conn, &agent_id));
                }
                DbCommand::GetSession { id, resp } => {
                    let _ = resp.send(repo::sessions::get(&conn, &id));
                }
                DbCommand::InsertSession { row, resp } => {
                    let _ = resp.send(repo::sessions::insert(&conn, &row));
                }
                DbCommand::UpdateSessionTitle { id, title, resp } => {
                    let _ = resp.send(repo::sessions::update_title(&conn, &id, &title));
                }
                DbCommand::UpdateSessionSummary { id, summary, resp } => {
                    let _ = resp.send(repo::sessions::update_summary(&conn, &id, &summary));
                }
                DbCommand::DeleteSession { id, resp } => {
                    let _ = resp.send(repo::sessions::delete(&conn, &id));
                }
                DbCommand::SetSessionPin { id, pinned, resp } => {
                    let _ = resp.send(repo::sessions::set_pin(&conn, &id, pinned));
                }
                DbCommand::UpdateSessionLlm { id, model, thinking_mode, thinking_budget, resp } => {
                    let _ = resp.send(repo::sessions::update_llm(
                        &conn,
                        &id,
                        &model,
                        &thinking_mode,
                        thinking_budget,
                    ));
                }
                DbCommand::ListMessagesWithParts { session_id, resp } => {
                    let _ = resp.send(repo::messages::list_with_parts(&conn, &session_id));
                }
                DbCommand::InsertMessage { msg, parts, resp } => {
                    let transaction_res = (|| -> AppResult<()> {
                        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        repo::messages::insert(&tx, &msg)?;
                        for part in &parts {
                            repo::messages::insert_part(&tx, part)?;
                        }
                        tx.commit()?;
                        Ok(())
                    })();
                    let _ = resp.send(transaction_res);
                }
                DbCommand::InsertMessageParts { parts, resp } => {
                    let res = (|| -> AppResult<()> {
                        for p in parts {
                            repo::messages::insert_part(&conn, &p)?;
                        }
                        Ok(())
                    })();
                    let _ = resp.send(res);
                }
                DbCommand::UpdateMessageStatus { id, status, resp } => {
                    let _ = resp.send(repo::messages::update_status(&conn, &id, &status));
                }
                DbCommand::InsertToolCall { row, resp } => {
                    let _ = resp.send(repo::tools::insert(&conn, &row));
                }
                DbCommand::UpdateToolCallRunning { id, cwd, resp } => {
                    let _ = resp.send(repo::tools::update_running(&conn, &id, &cwd));
                }
                DbCommand::UpdateToolCallComplete {
                    id,
                    status,
                    result,
                    exit_code,
                    stdout,
                    stderr,
                    resp,
                } => {
                    let _ = resp.send(repo::tools::update_complete(
                        &conn,
                        &id,
                        &status,
                        result.as_deref(),
                        exit_code,
                        stdout.as_deref(),
                        stderr.as_deref(),
                    ));
                }
                DbCommand::GetToolCall { id, resp } => {
                    let _ = resp.send(repo::tools::get(&conn, &id));
                }
                DbCommand::ListToolCalls { session_id, resp } => {
                    let _ = resp.send(repo::tools::list_for_session(&conn, &session_id));
                }
                DbCommand::ListModelProviders { resp } => {
                    let _ = resp.send(repo::model_providers::list(&conn));
                }
                DbCommand::GetModelProvider { id, resp } => {
                    let _ = resp.send(repo::model_providers::get(&conn, &id));
                }
                DbCommand::UpsertModelProvider { row, set_default, resp } => {
                    let res = (|| -> AppResult<String> {
                        if set_default {
                            repo::model_providers::clear_default(&conn)?;
                        }
                        repo::model_providers::upsert(&conn, &row)
                    })();
                    let _ = resp.send(res);
                }
                DbCommand::DeleteModelProvider { id, resp } => {
                    let _ = resp.send(repo::model_providers::delete(&conn, &id));
                }
                DbCommand::GetDefaultModelProvider { resp } => {
                    let _ = resp.send(repo::model_providers::get_default(&conn));
                }
            }
        }
    });
    DbActorHandle { tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_db_actor_full_flow() {
        let db_path = PathBuf::from("target/test_agnes.db");
        if db_path.exists() {
            let _ = fs::remove_file(&db_path);
        }

        let handle = spawn(db_path.clone());

        // 1. Insert Agent
        let agent = repo::agents::NewAgent {
            id: "test-agent".into(),
            name: "Test Agent".into(),
            persona: "You are a test agent".into(),
            scenario: String::new(),
            system_prompt: "Be a test agent".into(),
            greeting: String::new(),
            example_dialogue: String::new(),
            model: "GPT-4".into(),
            tool_policy: "{}".into(),
            avatar: String::new(),
            tags: String::new(),
            thinking_mode: "off".into(),
            thinking_budget: 0,
        };
        let agent_id = handle.insert_agent(agent).await.unwrap();
        assert_eq!(agent_id, "test-agent");

        let agents = handle.list_agents().await.unwrap();
        assert_eq!(agents.len(), 4);
        let test_agent_row = agents.iter().find(|a| a.id == "test-agent").unwrap();
        assert_eq!(test_agent_row.name, "Test Agent");

        // 2. Insert Session
        let session = repo::sessions::NewSession {
            id: "test-session".into(),
            agent_id: "test-agent".into(),
            title: "Test Title".into(),
            context_limit: None,
            compress_threshold: Some(0.8),
            recency_window: Some(15),
            reserved_output_tokens: None,
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            origin_device_id: None,
        };
        let sess_id = handle.insert_session(session).await.unwrap();
        assert_eq!(sess_id, "test-session");

        let sessions = handle.list_sessions("test-agent".into()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Test Title");

        // Get Session
        let got_sess = handle.get_session("test-session".into()).await.unwrap().unwrap();
        assert_eq!(got_sess.title, "Test Title");
        assert_eq!(got_sess.recency_window, 15);

        // Update title
        handle.update_session_title("test-session".into(), "New Title".into()).await.unwrap();
        let got_sess = handle.get_session("test-session".into()).await.unwrap().unwrap();
        assert_eq!(got_sess.title, "New Title");

        // Update summary
        handle.update_session_summary("test-session".into(), "Session Summary".into()).await.unwrap();
        let got_sess = handle.get_session("test-session".into()).await.unwrap().unwrap();
        assert_eq!(got_sess.summary.unwrap(), "Session Summary");

        // 3. Insert Messages
        let msg = repo::messages::NewMessage {
            id: "msg-1".into(),
            session_id: "test-session".into(),
            role: "user".into(),
            seq: 0,
            status: "complete".into(),
            model: None,
            token_count: None,
            metadata: None,
        };
        let part1 = repo::messages::NewMessagePart {
            id: "part-1".into(),
            message_id: "msg-1".into(),
            kind: "text".into(),
            ordinal: 0,
            mime_type: None,
            tool_call_id: None,
            content: "Hello Agent!".into(),
            metadata: None,
        };
        handle.insert_message(msg, vec![part1]).await.unwrap();

        let msg_with_parts = handle.list_messages_with_parts("test-session".into()).await.unwrap();
        assert_eq!(msg_with_parts.len(), 1);
        assert_eq!(msg_with_parts[0].0.id, "msg-1");
        assert_eq!(msg_with_parts[0].1.len(), 1);
        assert_eq!(msg_with_parts[0].1[0].content, "Hello Agent!");

        // Update message status
        handle.update_message_status("msg-1".into(), "failed".into()).await.unwrap();
        let msg_with_parts = handle.list_messages_with_parts("test-session".into()).await.unwrap();
        assert_eq!(msg_with_parts[0].0.status, "failed");

        // 4. Insert Tool Call
        let tool_call = repo::tools::NewToolCall {
            id: "tc-1".into(),
            session_id: "test-session".into(),
            message_id: Some("msg-1".into()),
            tool: "shell".into(),
            params: Some("ls".into()),
            status: "pending_approval".into(),
            risk_level: Some("Medium".into()),
            approval_policy_snapshot: None,
        };
        handle.insert_tool_call(tool_call).await.unwrap();

        let tc = handle.get_tool_call("tc-1".into()).await.unwrap().unwrap();
        assert_eq!(tc.tool, "shell");
        assert_eq!(tc.status, "pending_approval");

        // Update running
        handle.update_tool_call_running("tc-1".into(), "/projects".into()).await.unwrap();
        let tc = handle.get_tool_call("tc-1".into()).await.unwrap().unwrap();
        assert_eq!(tc.status, "running");
        assert_eq!(tc.cwd.unwrap(), "/projects");

        // Update complete
        handle.update_tool_call_complete(
            "tc-1".into(),
            "done".into(),
            Some("success".into()),
            Some(0),
            Some("file1.txt\n".into()),
            None,
        ).await.unwrap();
        let tc = handle.get_tool_call("tc-1".into()).await.unwrap().unwrap();
        assert_eq!(tc.status, "done");
        assert_eq!(tc.result.unwrap(), "success");
        assert_eq!(tc.stdout.unwrap(), "file1.txt\n");

        let tc_list = handle.list_tool_calls("test-session".into()).await.unwrap();
        assert_eq!(tc_list.len(), 1);
        assert_eq!(tc_list[0].id, "tc-1");

        // 5. Delete Session
        handle.delete_session("test-session".into()).await.unwrap();
        let sessions = handle.list_sessions("test-agent".into()).await.unwrap();
        // Since list_sessions filters out deleted_at IS NULL, it should be 0 now
        assert_eq!(sessions.len(), 0);

        let got_sess = handle.get_session("test-session".into()).await.unwrap().unwrap();
        assert!(got_sess.deleted_at.is_some());

        // 6. Test Settings
        handle.set_setting("test-key".into(), "test-value".into()).await.unwrap();
        let val = handle.get_setting("test-key".into()).await.unwrap();
        assert_eq!(val, Some("test-value".into()));

        // Cleanup
        let _ = fs::remove_file(&db_path);
    }
}

