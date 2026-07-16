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
        resp: oneshot::Sender<AppResult<bool>>,
    },
    ListMemories {
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::memory::MemoryRow>>>,
    },
    GetMemory {
        id: String,
        agent_id: String,
        resp: oneshot::Sender<AppResult<Option<repo::memory::MemoryRow>>>,
    },
    UpdateMemory {
        id: String,
        agent_id: String,
        changes: repo::memory::MemoryUpdate,
        resp: oneshot::Sender<AppResult<()>>,
    },
    DeleteMemory {
        id: String,
        agent_id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListExplicitMemories {
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::explicit_memories::ExplicitMemoryRow>>>,
    },
    SaveExplicitMemories {
        agent_id: String,
        user_id: String,
        user_md: String,
        memory_id: String,
        memory_md: String,
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
    DeleteSetting {
        key: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListSettingsWithPrefix {
        prefix: String,
        resp: oneshot::Sender<AppResult<Vec<(String, String)>>>,
    },
    UpsertMemoryEmbedding {
        embedding_id: String,
        memory_id: String,
        model: String,
        content: String,
        vector: Vec<f32>,
        resp: oneshot::Sender<AppResult<bool>>,
    },
    SearchMemories {
        query_text: String,
        agent_id: String,
        limit: usize,
        query_embedding: Option<repo::memory::QueryEmbedding>,
        resp: oneshot::Sender<AppResult<Vec<repo::memory::MemoryRow>>>,
    },
    ListKnowledgeCollections {
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::knowledge::KnowledgeCollectionRow>>>,
    },
    CreateKnowledgeCollection {
        row: repo::knowledge::NewKnowledgeCollection,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListKnowledgeDocuments {
        collection_id: String,
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::knowledge::KnowledgeDocumentRow>>>,
    },
    ImportLocalKnowledgeDocument {
        input: repo::knowledge::NewLocalDocument,
        resp: oneshot::Sender<AppResult<repo::knowledge::ImportDocumentResult>>,
    },
    SearchKnowledge {
        agent_id: String,
        query: String,
        collection_id: Option<String>,
        limit: usize,
        resp: oneshot::Sender<AppResult<Vec<repo::knowledge::KnowledgeSearchResult>>>,
    },
    ListCalendars {
        resp: oneshot::Sender<AppResult<Vec<repo::planner::CalendarRow>>>,
    },
    CreateCalendar {
        id: String,
        name: String,
        color: Option<String>,
        timezone: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListCalendarEvents {
        calendar_id: String,
        range_start: String,
        range_end: String,
        resp: oneshot::Sender<AppResult<Vec<repo::planner::EventRow>>>,
    },
    CreateCalendarEvent {
        id: String,
        calendar_id: String,
        title: String,
        starts_at: String,
        ends_at: String,
        timezone: String,
        all_day: bool,
        recurrence_rule: Option<String>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListTaskLists {
        resp: oneshot::Sender<AppResult<Vec<repo::planner::TaskListRow>>>,
    },
    CreateTaskList {
        id: String,
        name: String,
        color: Option<String>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListTasks {
        task_list_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::planner::TaskRow>>>,
    },
    CreateTask {
        id: String,
        task_list_id: String,
        parent_id: Option<String>,
        title: String,
        description: Option<String>,
        priority: i64,
        due_at: Option<String>,
        sort_order: f64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    CompleteTask {
        id: String,
        completed: bool,
        resp: oneshot::Sender<AppResult<()>>,
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
    UpdateSessionPermissionMode {
        id: String,
        permission_mode: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListWorkspaces {
        agent_id: String,
        resp: oneshot::Sender<AppResult<Vec<repo::workspaces::WorkspaceRow>>>,
    },
    GetWorkspace {
        id: String,
        resp: oneshot::Sender<AppResult<Option<repo::workspaces::WorkspaceRow>>>,
    },
    InsertWorkspace {
        row: repo::workspaces::NewWorkspace,
        resp: oneshot::Sender<AppResult<String>>,
    },
    RenameWorkspace {
        id: String,
        name: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    DeleteWorkspace {
        id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListMessagesWithParts {
        session_id: String,
        resp: oneshot::Sender<
            AppResult<
                Vec<(
                    repo::messages::MessageRow,
                    Vec<repo::messages::MessagePartRow>,
                )>,
            >,
        >,
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
    GetMessage {
        id: String,
        resp: oneshot::Sender<AppResult<Option<repo::messages::MessageRow>>>,
    },
    ListActiveWithParts {
        session_id: String,
        resp: oneshot::Sender<AppResult<repo::messages::ActivePathResult>>,
    },
    CountChildren {
        id: String,
        resp: oneshot::Sender<AppResult<u64>>,
    },
    SetSelectedChild {
        parent_id: String,
        child_id: Option<String>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    DeleteMessage {
        id: String,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ReplaceMessageParts {
        message_id: String,
        parts: Vec<repo::messages::NewMessagePart>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    AppendUserAndAssistant {
        session_id: String,
        parent_id: Option<String>,
        user_text: String,
        model: String,
        resp: oneshot::Sender<AppResult<(String, String)>>,
    },
    AppendAssistantSibling {
        session_id: String,
        parent_user_id: String,
        model: String,
        resp: oneshot::Sender<AppResult<String>>,
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
    GetSyncStatus {
        resp: oneshot::Sender<AppResult<repo::sync::SyncDbStatus>>,
    },
    SetSyncE2eeKeyVersion {
        key_version: Option<i64>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ListSyncConflicts {
        resp: oneshot::Sender<AppResult<Vec<repo::sync::SyncConflictRow>>>,
    },
    ResolveSyncConflict {
        conflict_id: String,
        resolution: String,
        now_ms: i64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ClaimSyncOutbox {
        limit: usize,
        now_ms: i64,
        resp: oneshot::Sender<AppResult<Vec<repo::sync::OutboxRow>>>,
    },
    PersistSealedSyncOutbox {
        changes: Vec<repo::sync::SealedOutboxChange>,
        resp: oneshot::Sender<AppResult<Vec<repo::sync::OutboxRow>>>,
    },
    ApplySyncPushResult {
        accepted: Vec<repo::sync::AcceptedChange>,
        conflicts: Vec<repo::sync::ConflictChange>,
        now_ms: i64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ApplySyncBootstrapPage {
        expected_state: String,
        entities: Vec<repo::sync::RemoteEntityInput>,
        snapshot_cursor: i64,
        next_cursor: Option<String>,
        has_more: bool,
        now_ms: i64,
        resp: oneshot::Sender<AppResult<i64>>,
    },
    ApplySyncPullPage {
        after: i64,
        entities: Vec<repo::sync::RemoteEntityInput>,
        next_cursor: i64,
        has_more: bool,
        now_ms: i64,
        resp: oneshot::Sender<AppResult<i64>>,
    },
    RecordSyncRuntimeFailure {
        error_code: String,
        backoff_until: Option<i64>,
        resp: oneshot::Sender<AppResult<()>>,
    },
    RecordSyncRuntimeSuccess {
        now_ms: i64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    ScheduleSyncRetry {
        change_ids: Vec<String>,
        error_code: String,
        error_message: String,
        retry_at: i64,
        resp: oneshot::Sender<AppResult<()>>,
    },
    MarkSyncDeadLetter {
        change_ids: Vec<String>,
        error_code: String,
        error_message: String,
        resp: oneshot::Sender<AppResult<()>>,
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
        self.send(DbCommand::UpdateAgentModel {
            agent_id,
            model,
            resp,
        })?;
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

    pub async fn insert_memory(&self, row: repo::memory::NewMemory) -> AppResult<bool> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertMemory { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_memories(&self, agent_id: String) -> AppResult<Vec<repo::memory::MemoryRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListMemories { agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_memory(
        &self,
        id: String,
        agent_id: String,
    ) -> AppResult<Option<repo::memory::MemoryRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetMemory { id, agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn update_memory(
        &self,
        id: String,
        agent_id: String,
        changes: repo::memory::MemoryUpdate,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateMemory {
            id,
            agent_id,
            changes,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_memory(&self, id: String, agent_id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteMemory { id, agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_explicit_memories(
        &self,
        agent_id: String,
    ) -> AppResult<Vec<repo::explicit_memories::ExplicitMemoryRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListExplicitMemories { agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn save_explicit_memories(
        &self,
        agent_id: String,
        user_md: String,
        memory_md: String,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SaveExplicitMemories {
            agent_id,
            user_id: uuid::Uuid::new_v4().to_string(),
            user_md,
            memory_id: uuid::Uuid::new_v4().to_string(),
            memory_md,
            resp,
        })?;
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

    pub async fn delete_setting(&self, key: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteSetting { key, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_settings_with_prefix(
        &self,
        prefix: String,
    ) -> AppResult<Vec<(String, String)>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListSettingsWithPrefix { prefix, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn upsert_memory_embedding(
        &self,
        embedding_id: String,
        memory_id: String,
        model: String,
        content: String,
        vector: Vec<f32>,
    ) -> AppResult<bool> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpsertMemoryEmbedding {
            embedding_id,
            memory_id,
            model,
            content,
            vector,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn search_memories(
        &self,
        query_text: String,
        agent_id: String,
        limit: usize,
        query_embedding: Option<repo::memory::QueryEmbedding>,
    ) -> AppResult<Vec<repo::memory::MemoryRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SearchMemories {
            query_text,
            agent_id,
            limit,
            query_embedding,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_knowledge_collections(
        &self,
        agent_id: String,
    ) -> AppResult<Vec<repo::knowledge::KnowledgeCollectionRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListKnowledgeCollections { agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn create_knowledge_collection(
        &self,
        row: repo::knowledge::NewKnowledgeCollection,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CreateKnowledgeCollection { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_knowledge_documents(
        &self,
        collection_id: String,
        agent_id: String,
    ) -> AppResult<Vec<repo::knowledge::KnowledgeDocumentRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListKnowledgeDocuments {
            collection_id,
            agent_id,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn import_local_knowledge_document(
        &self,
        input: repo::knowledge::NewLocalDocument,
    ) -> AppResult<repo::knowledge::ImportDocumentResult> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ImportLocalKnowledgeDocument { input, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn search_knowledge(
        &self,
        agent_id: String,
        query: String,
        collection_id: Option<String>,
        limit: usize,
    ) -> AppResult<Vec<repo::knowledge::KnowledgeSearchResult>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SearchKnowledge {
            agent_id,
            query,
            collection_id,
            limit,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_calendars(&self) -> AppResult<Vec<repo::planner::CalendarRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListCalendars { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn create_calendar(
        &self,
        id: String,
        name: String,
        color: Option<String>,
        timezone: String,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CreateCalendar {
            id,
            name,
            color,
            timezone,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn list_calendar_events(
        &self,
        calendar_id: String,
        range_start: String,
        range_end: String,
    ) -> AppResult<Vec<repo::planner::EventRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListCalendarEvents {
            calendar_id,
            range_start,
            range_end,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn create_calendar_event(
        &self,
        id: String,
        calendar_id: String,
        title: String,
        starts_at: String,
        ends_at: String,
        timezone: String,
        all_day: bool,
        recurrence_rule: Option<String>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CreateCalendarEvent {
            id,
            calendar_id,
            title,
            starts_at,
            ends_at,
            timezone,
            all_day,
            recurrence_rule,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn list_task_lists(&self) -> AppResult<Vec<repo::planner::TaskListRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListTaskLists { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn create_task_list(
        &self,
        id: String,
        name: String,
        color: Option<String>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CreateTaskList {
            id,
            name,
            color,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn list_tasks(&self, task_list_id: String) -> AppResult<Vec<repo::planner::TaskRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListTasks { task_list_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn create_task(
        &self,
        id: String,
        task_list_id: String,
        parent_id: Option<String>,
        title: String,
        description: Option<String>,
        priority: i64,
        due_at: Option<String>,
        sort_order: f64,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CreateTask {
            id,
            task_list_id,
            parent_id,
            title,
            description,
            priority,
            due_at,
            sort_order,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }
    pub async fn complete_task(&self, id: String, completed: bool) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CompleteTask {
            id,
            completed,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_sessions(
        &self,
        agent_id: String,
    ) -> AppResult<Vec<repo::sessions::SessionRow>> {
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

    pub async fn update_session_permission_mode(
        &self,
        id: String,
        permission_mode: String,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpdateSessionPermissionMode {
            id,
            permission_mode,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_workspaces(
        &self,
        agent_id: String,
    ) -> AppResult<Vec<repo::workspaces::WorkspaceRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListWorkspaces { agent_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_workspace(
        &self,
        id: String,
    ) -> AppResult<Option<repo::workspaces::WorkspaceRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetWorkspace { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn insert_workspace(&self, row: repo::workspaces::NewWorkspace) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::InsertWorkspace { row, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn rename_workspace(&self, id: String, name: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::RenameWorkspace { id, name, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_workspace(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteWorkspace { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_messages_with_parts(
        &self,
        session_id: String,
    ) -> AppResult<
        Vec<(
            repo::messages::MessageRow,
            Vec<repo::messages::MessagePartRow>,
        )>,
    > {
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

    pub async fn get_message(&self, id: String) -> AppResult<Option<repo::messages::MessageRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetMessage { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_active_with_parts(
        &self,
        session_id: String,
    ) -> AppResult<repo::messages::ActivePathResult> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListActiveWithParts { session_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn count_children(&self, id: String) -> AppResult<u64> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::CountChildren { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn set_selected_child(
        &self,
        parent_id: String,
        child_id: Option<String>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SetSelectedChild {
            parent_id,
            child_id,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_message(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteMessage { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn replace_message_parts(
        &self,
        message_id: String,
        parts: Vec<repo::messages::NewMessagePart>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ReplaceMessageParts {
            message_id,
            parts,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn append_user_and_assistant(
        &self,
        session_id: String,
        parent_id: Option<String>,
        user_text: String,
        model: String,
    ) -> AppResult<(String, String)> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::AppendUserAndAssistant {
            session_id,
            parent_id,
            user_text,
            model,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn append_assistant_sibling(
        &self,
        session_id: String,
        parent_user_id: String,
        model: String,
    ) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::AppendAssistantSibling {
            session_id,
            parent_user_id,
            model,
            resp,
        })?;
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

    pub async fn list_tool_calls(
        &self,
        session_id: String,
    ) -> AppResult<Vec<repo::tools::ToolCallRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListToolCalls { session_id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_model_providers(
        &self,
    ) -> AppResult<Vec<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListModelProviders { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_model_provider(
        &self,
        id: String,
    ) -> AppResult<Option<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetModelProvider { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn upsert_model_provider(
        &self,
        row: repo::model_providers::NewModelProvider,
        set_default: bool,
    ) -> AppResult<String> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::UpsertModelProvider {
            row,
            set_default,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn delete_model_provider(&self, id: String) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::DeleteModelProvider { id, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_default_model_provider(
        &self,
    ) -> AppResult<Option<repo::model_providers::ModelProviderRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetDefaultModelProvider { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn get_sync_status(&self) -> AppResult<repo::sync::SyncDbStatus> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::GetSyncStatus { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn set_sync_e2ee_key_version(&self, key_version: Option<i64>) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::SetSyncE2eeKeyVersion { key_version, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn list_sync_conflicts(&self) -> AppResult<Vec<repo::sync::SyncConflictRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ListSyncConflicts { resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn resolve_sync_conflict(
        &self,
        conflict_id: String,
        resolution: String,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        self.send(DbCommand::ResolveSyncConflict {
            conflict_id,
            resolution,
            now_ms,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn claim_sync_outbox(
        &self,
        limit: usize,
        now_ms: i64,
    ) -> AppResult<Vec<repo::sync::OutboxRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ClaimSyncOutbox {
            limit,
            now_ms,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn persist_sealed_sync_outbox(
        &self,
        changes: Vec<repo::sync::SealedOutboxChange>,
    ) -> AppResult<Vec<repo::sync::OutboxRow>> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::PersistSealedSyncOutbox { changes, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn apply_sync_push_result(
        &self,
        accepted: Vec<repo::sync::AcceptedChange>,
        conflicts: Vec<repo::sync::ConflictChange>,
        now_ms: i64,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ApplySyncPushResult {
            accepted,
            conflicts,
            now_ms,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn apply_sync_bootstrap_page(
        &self,
        expected_state: String,
        entities: Vec<repo::sync::RemoteEntityInput>,
        snapshot_cursor: i64,
        next_cursor: Option<String>,
        has_more: bool,
        now_ms: i64,
    ) -> AppResult<i64> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ApplySyncBootstrapPage {
            expected_state,
            entities,
            snapshot_cursor,
            next_cursor,
            has_more,
            now_ms,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn apply_sync_pull_page(
        &self,
        after: i64,
        entities: Vec<repo::sync::RemoteEntityInput>,
        next_cursor: i64,
        has_more: bool,
        now_ms: i64,
    ) -> AppResult<i64> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ApplySyncPullPage {
            after,
            entities,
            next_cursor,
            has_more,
            now_ms,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn record_sync_runtime_failure(
        &self,
        error_code: String,
        backoff_until: Option<i64>,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::RecordSyncRuntimeFailure {
            error_code,
            backoff_until,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn record_sync_runtime_success(&self, now_ms: i64) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::RecordSyncRuntimeSuccess { now_ms, resp })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn schedule_sync_retry(
        &self,
        change_ids: Vec<String>,
        error_code: String,
        error_message: String,
        retry_at: i64,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::ScheduleSyncRetry {
            change_ids,
            error_code,
            error_message,
            retry_at,
            resp,
        })?;
        rx.await
            .map_err(|_| AppError::Other("db actor 已丢弃".into()))?
    }

    pub async fn mark_sync_dead_letter(
        &self,
        change_ids: Vec<String>,
        error_code: String,
        error_message: String,
    ) -> AppResult<()> {
        let (resp, rx) = oneshot::channel();
        self.send(DbCommand::MarkSyncDeadLetter {
            change_ids,
            error_code,
            error_message,
            resp,
        })?;
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
                    let _ = resp.send(repo::agents::insert(&mut conn, &row));
                }
                DbCommand::UpdateAgentModel {
                    agent_id,
                    model,
                    resp,
                } => {
                    let _ = resp.send(repo::agents::update_model(&mut conn, &agent_id, &model));
                }
                DbCommand::UpdateAgent { id, changes, resp } => {
                    let _ = resp.send(repo::agents::update(&mut conn, &id, &changes));
                }
                DbCommand::DeleteAgent { id, resp } => {
                    let _ = resp.send(repo::agents::delete(&mut conn, &id));
                }
                DbCommand::InsertMemory { row, resp } => {
                    let _ = resp.send(repo::memory::insert(&mut conn, &row));
                }
                DbCommand::ListMemories { agent_id, resp } => {
                    let _ = resp.send(repo::memory::list(&conn, &agent_id));
                }
                DbCommand::GetMemory { id, agent_id, resp } => {
                    let _ = resp.send(repo::memory::get(&conn, &id, &agent_id));
                }
                DbCommand::UpdateMemory {
                    id,
                    agent_id,
                    changes,
                    resp,
                } => {
                    let _ = resp.send(repo::memory::update(&mut conn, &id, &agent_id, &changes));
                }
                DbCommand::DeleteMemory { id, agent_id, resp } => {
                    let _ = resp.send(repo::memory::delete(&mut conn, &id, &agent_id));
                }
                DbCommand::ListExplicitMemories { agent_id, resp } => {
                    let _ = resp.send(repo::explicit_memories::list(&conn, &agent_id));
                }
                DbCommand::SaveExplicitMemories {
                    agent_id,
                    user_id,
                    user_md,
                    memory_id,
                    memory_md,
                    resp,
                } => {
                    let _ = resp.send(repo::explicit_memories::save_pair(
                        &mut conn, &agent_id, &user_id, &user_md, &memory_id, &memory_md,
                    ));
                }
                DbCommand::GetSetting { key, resp } => {
                    let _ = resp.send(repo::settings::get(&conn, &key));
                }
                DbCommand::SetSetting { key, value, resp } => {
                    let _ = resp.send(repo::settings::set(&conn, &key, &value));
                }
                DbCommand::DeleteSetting { key, resp } => {
                    let _ = resp.send(repo::settings::delete(&conn, &key));
                }
                DbCommand::ListSettingsWithPrefix { prefix, resp } => {
                    let _ = resp.send(repo::settings::list_with_prefix(&conn, &prefix));
                }
                DbCommand::UpsertMemoryEmbedding {
                    embedding_id,
                    memory_id,
                    model,
                    content,
                    vector,
                    resp,
                } => {
                    let _ = resp.send(repo::memory::upsert_memory_embedding(
                        &mut conn,
                        &embedding_id,
                        &memory_id,
                        &model,
                        &content,
                        &vector,
                    ));
                }
                DbCommand::SearchMemories {
                    query_text,
                    agent_id,
                    limit,
                    query_embedding,
                    resp,
                } => {
                    let _ = resp.send(repo::memory::search_hybrid(
                        &conn,
                        &query_text,
                        &agent_id,
                        limit,
                        query_embedding.as_ref(),
                    ));
                }
                DbCommand::ListKnowledgeCollections { agent_id, resp } => {
                    let _ = resp.send(repo::knowledge::list_collections(&conn, &agent_id));
                }
                DbCommand::CreateKnowledgeCollection { row, resp } => {
                    let _ = resp.send(repo::knowledge::create_collection(&mut conn, &row));
                }
                DbCommand::ListKnowledgeDocuments {
                    collection_id,
                    agent_id,
                    resp,
                } => {
                    let _ = resp.send(repo::knowledge::list_documents(
                        &conn,
                        &collection_id,
                        &agent_id,
                    ));
                }
                DbCommand::ImportLocalKnowledgeDocument { input, resp } => {
                    let _ = resp.send(repo::knowledge::import_local_document(&mut conn, &input));
                }
                DbCommand::SearchKnowledge {
                    agent_id,
                    query,
                    collection_id,
                    limit,
                    resp,
                } => {
                    let _ = resp.send(repo::knowledge::search(
                        &conn,
                        &agent_id,
                        &query,
                        collection_id.as_deref(),
                        limit,
                    ));
                }
                DbCommand::ListCalendars { resp } => {
                    let _ = resp.send(repo::planner::list_calendars(&conn));
                }
                DbCommand::CreateCalendar {
                    id,
                    name,
                    color,
                    timezone,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::create_calendar(
                        &conn, &id, &name, color, &timezone,
                    ));
                }
                DbCommand::ListCalendarEvents {
                    calendar_id,
                    range_start,
                    range_end,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::list_events(
                        &conn,
                        &calendar_id,
                        &range_start,
                        &range_end,
                    ));
                }
                DbCommand::CreateCalendarEvent {
                    id,
                    calendar_id,
                    title,
                    starts_at,
                    ends_at,
                    timezone,
                    all_day,
                    recurrence_rule,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::create_event(
                        &conn,
                        &id,
                        &calendar_id,
                        &title,
                        &starts_at,
                        &ends_at,
                        &timezone,
                        all_day,
                        recurrence_rule,
                    ));
                }
                DbCommand::ListTaskLists { resp } => {
                    let _ = resp.send(repo::planner::list_task_lists(&conn));
                }
                DbCommand::CreateTaskList {
                    id,
                    name,
                    color,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::create_task_list(&conn, &id, &name, color));
                }
                DbCommand::ListTasks { task_list_id, resp } => {
                    let _ = resp.send(repo::planner::list_tasks(&conn, &task_list_id));
                }
                DbCommand::CreateTask {
                    id,
                    task_list_id,
                    parent_id,
                    title,
                    description,
                    priority,
                    due_at,
                    sort_order,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::create_task(
                        &conn,
                        &id,
                        &task_list_id,
                        parent_id,
                        &title,
                        description,
                        priority,
                        due_at,
                        sort_order,
                    ));
                }
                DbCommand::CompleteTask {
                    id,
                    completed,
                    resp,
                } => {
                    let _ = resp.send(repo::planner::complete_task(&conn, &id, completed));
                }
                DbCommand::ListSessions { agent_id, resp } => {
                    let _ = resp.send(repo::sessions::list(&conn, &agent_id));
                }
                DbCommand::GetSession { id, resp } => {
                    let _ = resp.send(repo::sessions::get(&conn, &id));
                }
                DbCommand::InsertSession { row, resp } => {
                    let _ = resp.send(repo::sessions::insert(&mut conn, &row));
                }
                DbCommand::UpdateSessionTitle { id, title, resp } => {
                    let _ = resp.send(repo::sessions::update_title(&mut conn, &id, &title));
                }
                DbCommand::UpdateSessionSummary { id, summary, resp } => {
                    let _ = resp.send(repo::sessions::update_summary(&mut conn, &id, &summary));
                }
                DbCommand::DeleteSession { id, resp } => {
                    let _ = resp.send(repo::sessions::delete(&mut conn, &id));
                }
                DbCommand::SetSessionPin { id, pinned, resp } => {
                    let _ = resp.send(repo::sessions::set_pin(&mut conn, &id, pinned));
                }
                DbCommand::UpdateSessionLlm {
                    id,
                    model,
                    thinking_mode,
                    thinking_budget,
                    resp,
                } => {
                    let _ = resp.send(repo::sessions::update_llm(
                        &mut conn,
                        &id,
                        &model,
                        &thinking_mode,
                        thinking_budget,
                    ));
                }
                DbCommand::UpdateSessionPermissionMode {
                    id,
                    permission_mode,
                    resp,
                } => {
                    let _ = resp.send(repo::sessions::update_permission_mode(
                        &conn,
                        &id,
                        &permission_mode,
                    ));
                }
                DbCommand::ListWorkspaces { agent_id, resp } => {
                    let _ = resp.send(repo::workspaces::list(&conn, &agent_id));
                }
                DbCommand::GetWorkspace { id, resp } => {
                    let _ = resp.send(repo::workspaces::get(&conn, &id));
                }
                DbCommand::InsertWorkspace { row, resp } => {
                    let _ = resp.send(repo::workspaces::insert(&mut conn, &row));
                }
                DbCommand::RenameWorkspace { id, name, resp } => {
                    let _ = resp.send(repo::workspaces::rename(&mut conn, &id, &name));
                }
                DbCommand::DeleteWorkspace { id, resp } => {
                    let _ = resp.send(repo::workspaces::delete(&mut conn, &id));
                }
                DbCommand::ListMessagesWithParts { session_id, resp } => {
                    let _ = resp.send(repo::messages::list_with_parts(&conn, &session_id));
                }
                DbCommand::InsertMessage { msg, parts, resp } => {
                    let transaction_res = (|| -> AppResult<()> {
                        let tx = conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        repo::messages::insert(&tx, &msg)?;
                        for part in &parts {
                            repo::messages::insert_part(&tx, part)?;
                        }
                        repo::messages::enqueue_if_complete(&tx, &msg.id)?;
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
                    let _ = resp.send(repo::messages::update_status(&mut conn, &id, &status));
                }
                DbCommand::GetMessage { id, resp } => {
                    let _ = resp.send(repo::messages::get(&conn, &id));
                }
                DbCommand::ListActiveWithParts { session_id, resp } => {
                    let _ = resp.send(repo::messages::list_active_with_parts(&conn, &session_id));
                }
                DbCommand::CountChildren { id, resp } => {
                    let _ = resp.send(repo::messages::count_children(&conn, &id));
                }
                DbCommand::SetSelectedChild {
                    parent_id,
                    child_id,
                    resp,
                } => {
                    let _ = resp.send(repo::messages::set_selected_child(
                        &mut conn,
                        &parent_id,
                        child_id.as_deref(),
                    ));
                }
                DbCommand::DeleteMessage { id, resp } => {
                    let _ = resp.send(repo::messages::delete_message(&mut conn, &id));
                }
                DbCommand::ReplaceMessageParts {
                    message_id,
                    parts,
                    resp,
                } => {
                    let transaction_res = (|| -> AppResult<()> {
                        let tx = conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        repo::messages::delete_parts(&tx, &message_id)?;
                        for (i, mut p) in parts.into_iter().enumerate() {
                            p.ordinal = i as i32;
                            repo::messages::insert_part(&tx, &p)?;
                        }
                        repo::messages::mark_content_updated(&tx, &message_id)?;
                        repo::messages::enqueue_if_complete(&tx, &message_id)?;
                        tx.commit()?;
                        Ok(())
                    })();
                    let _ = resp.send(transaction_res);
                }
                DbCommand::AppendUserAndAssistant {
                    session_id,
                    parent_id,
                    user_text,
                    model,
                    resp,
                } => {
                    let transaction_res = (|| -> AppResult<(String, String)> {
                        let tx = conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        let user_id = uuid::Uuid::new_v4().to_string();
                        let seq_u = repo::messages::get_next_seq(&tx, &session_id)?;
                        repo::messages::insert(
                            &tx,
                            &repo::messages::NewMessage {
                                id: user_id.clone(),
                                session_id: session_id.clone(),
                                role: "user".into(),
                                seq: seq_u,
                                status: "complete".into(),
                                model: None,
                                token_count: None,
                                metadata: None,
                                parent_id: parent_id.clone(),
                                selected_child_id: None,
                            },
                        )?;
                        let user_part_id = uuid::Uuid::new_v4().to_string();
                        repo::messages::insert_part(
                            &tx,
                            &repo::messages::NewMessagePart {
                                id: user_part_id,
                                message_id: user_id.clone(),
                                kind: "text".into(),
                                ordinal: 0,
                                mime_type: None,
                                tool_call_id: None,
                                content: user_text,
                                metadata: None,
                            },
                        )?;
                        let ai_id = uuid::Uuid::new_v4().to_string();
                        let seq_a = repo::messages::get_next_seq(&tx, &session_id)?;
                        repo::messages::insert(
                            &tx,
                            &repo::messages::NewMessage {
                                id: ai_id.clone(),
                                session_id: session_id.clone(),
                                role: "assistant".into(),
                                seq: seq_a,
                                status: "pending".into(),
                                model: Some(model.clone()),
                                token_count: None,
                                metadata: None,
                                parent_id: Some(user_id.clone()),
                                selected_child_id: None,
                            },
                        )?;
                        // 链接：父→user→ai
                        if let Some(ref pid) = parent_id {
                            repo::messages::set_selected_child_local(&tx, pid, Some(&user_id))?;
                            repo::messages::enqueue_if_complete(&tx, pid)?;
                        }
                        repo::messages::set_selected_child_local(&tx, &user_id, Some(&ai_id))?;
                        repo::messages::enqueue_if_complete(&tx, &user_id)?;
                        tx.commit()?;
                        Ok((user_id, ai_id))
                    })();
                    let _ = resp.send(transaction_res);
                }
                DbCommand::AppendAssistantSibling {
                    session_id,
                    parent_user_id,
                    model,
                    resp,
                } => {
                    let transaction_res = (|| -> AppResult<String> {
                        let tx = conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        let ai_id = uuid::Uuid::new_v4().to_string();
                        let seq_a = repo::messages::get_next_seq(&tx, &session_id)?;
                        repo::messages::insert(
                            &tx,
                            &repo::messages::NewMessage {
                                id: ai_id.clone(),
                                session_id: session_id.clone(),
                                role: "assistant".into(),
                                seq: seq_a,
                                status: "pending".into(),
                                model: Some(model.clone()),
                                token_count: None,
                                metadata: None,
                                parent_id: Some(parent_user_id.clone()),
                                selected_child_id: None,
                            },
                        )?;
                        // 把活动路径切到新 AI 同级
                        repo::messages::set_selected_child_local(
                            &tx,
                            &parent_user_id,
                            Some(&ai_id),
                        )?;
                        repo::messages::enqueue_if_complete(&tx, &parent_user_id)?;
                        tx.commit()?;
                        Ok(ai_id)
                    })();
                    let _ = resp.send(transaction_res);
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
                DbCommand::UpsertModelProvider {
                    row,
                    set_default,
                    resp,
                } => {
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
                DbCommand::GetSyncStatus { resp } => {
                    let _ = resp.send(repo::sync::status(&conn));
                }
                DbCommand::SetSyncE2eeKeyVersion { key_version, resp } => {
                    let _ = resp.send(repo::sync::set_e2ee_key_version(&conn, key_version));
                }
                DbCommand::ListSyncConflicts { resp } => {
                    let _ = resp.send(repo::sync::list_conflicts(&conn));
                }
                DbCommand::ResolveSyncConflict {
                    conflict_id,
                    resolution,
                    now_ms,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::resolve_conflict(
                        &mut conn,
                        &conflict_id,
                        &resolution,
                        now_ms,
                    ));
                }
                DbCommand::ClaimSyncOutbox {
                    limit,
                    now_ms,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::claim_pending(&mut conn, limit, now_ms));
                }
                DbCommand::PersistSealedSyncOutbox { changes, resp } => {
                    let _ = resp.send(repo::sync::persist_sealed_outbox(&mut conn, &changes));
                }
                DbCommand::ApplySyncPushResult {
                    accepted,
                    conflicts,
                    now_ms,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::apply_push_result(
                        &mut conn, &accepted, &conflicts, now_ms,
                    ));
                }
                DbCommand::ApplySyncBootstrapPage {
                    expected_state,
                    entities,
                    snapshot_cursor,
                    next_cursor,
                    has_more,
                    now_ms,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::apply_bootstrap_page(
                        &mut conn,
                        &expected_state,
                        &entities,
                        snapshot_cursor,
                        next_cursor.as_deref(),
                        has_more,
                        now_ms,
                    ));
                }
                DbCommand::ApplySyncPullPage {
                    after,
                    entities,
                    next_cursor,
                    has_more,
                    now_ms,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::apply_pull_page(
                        &mut conn,
                        after,
                        &entities,
                        next_cursor,
                        has_more,
                        now_ms,
                    ));
                }
                DbCommand::RecordSyncRuntimeFailure {
                    error_code,
                    backoff_until,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::record_runtime_failure(
                        &conn,
                        &error_code,
                        backoff_until,
                    ));
                }
                DbCommand::RecordSyncRuntimeSuccess { now_ms, resp } => {
                    let _ = resp.send(repo::sync::record_runtime_success(&conn, now_ms));
                }
                DbCommand::ScheduleSyncRetry {
                    change_ids,
                    error_code,
                    error_message,
                    retry_at,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::schedule_retry(
                        &mut conn,
                        &change_ids,
                        &error_code,
                        &error_message,
                        retry_at,
                    ));
                }
                DbCommand::MarkSyncDeadLetter {
                    change_ids,
                    error_code,
                    error_message,
                    resp,
                } => {
                    let _ = resp.send(repo::sync::mark_dead_letter(
                        &mut conn,
                        &change_ids,
                        &error_code,
                        &error_message,
                    ));
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
            permission_mode: "auto".into(),
            workspace_id: None,
            origin_device_id: None,
        };
        let sess_id = handle.insert_session(session).await.unwrap();
        assert_eq!(sess_id, "test-session");

        let sessions = handle.list_sessions("test-agent".into()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Test Title");

        // Get Session
        let got_sess = handle
            .get_session("test-session".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got_sess.title, "Test Title");
        assert_eq!(got_sess.recency_window, 15);
        assert_eq!(got_sess.permission_mode, "auto");

        handle
            .update_session_permission_mode("test-session".into(), "accept_edits".into())
            .await
            .unwrap();
        let got_sess = handle
            .get_session("test-session".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got_sess.permission_mode, "accept_edits");

        // Update title
        handle
            .update_session_title("test-session".into(), "New Title".into())
            .await
            .unwrap();
        let got_sess = handle
            .get_session("test-session".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got_sess.title, "New Title");

        // Update summary
        handle
            .update_session_summary("test-session".into(), "Session Summary".into())
            .await
            .unwrap();
        let got_sess = handle
            .get_session("test-session".into())
            .await
            .unwrap()
            .unwrap();
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
            parent_id: None,
            selected_child_id: None,
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

        let msg_with_parts = handle
            .list_messages_with_parts("test-session".into())
            .await
            .unwrap();
        assert_eq!(msg_with_parts.len(), 1);
        assert_eq!(msg_with_parts[0].0.id, "msg-1");
        assert_eq!(msg_with_parts[0].1.len(), 1);
        assert_eq!(msg_with_parts[0].1[0].content, "Hello Agent!");

        // Update message status
        handle
            .update_message_status("msg-1".into(), "failed".into())
            .await
            .unwrap();
        let msg_with_parts = handle
            .list_messages_with_parts("test-session".into())
            .await
            .unwrap();
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
        handle
            .update_tool_call_running("tc-1".into(), "/projects".into())
            .await
            .unwrap();
        let tc = handle.get_tool_call("tc-1".into()).await.unwrap().unwrap();
        assert_eq!(tc.status, "running");
        assert_eq!(tc.cwd.unwrap(), "/projects");

        // Update complete
        handle
            .update_tool_call_complete(
                "tc-1".into(),
                "done".into(),
                Some("success".into()),
                Some(0),
                Some("file1.txt\n".into()),
                None,
            )
            .await
            .unwrap();
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

        let got_sess = handle
            .get_session("test-session".into())
            .await
            .unwrap()
            .unwrap();
        assert!(got_sess.deleted_at.is_some());

        // 6. Test Settings
        handle
            .set_setting("test-key".into(), "test-value".into())
            .await
            .unwrap();
        let val = handle.get_setting("test-key".into()).await.unwrap();
        assert_eq!(val, Some("test-value".into()));

        // Cleanup
        let _ = fs::remove_file(&db_path);
    }
}
