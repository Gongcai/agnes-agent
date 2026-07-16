use serde::{Deserialize, Serialize};

/// Agent Protocol 版本。Rust 结构体为真相源；Python 用 pydantic 镜像，TS 由 specta/tauri-typegen 生成。
pub const PROTOCOL_VERSION: u8 = 1;

/// 所有协议消息的统一信封（每个帧必备基础字段）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Envelope {
    pub protocol_version: u8,
    pub id: String,
    pub run_id: String,
    pub session_id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub created_at: String,
    pub payload: serde_json::Value,
}

impl Envelope {
    /// 构造一个回包信封（V0.1 骨架：握手用，created_at 留空，会话/运行上下文在 V0.2 填充）。
    pub fn reply(msg_type: &str, payload: serde_json::Value) -> Self {
        Envelope {
            protocol_version: PROTOCOL_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            run_id: String::new(),
            session_id: String::new(),
            msg_type: msg_type.to_string(),
            created_at: String::new(),
            payload,
        }
    }
}

/// 消息类型（方向在架构文档 §3.2 已定死）：
/// Rust→Python: run_request / tool_result / approval_result / run_cancel / user_message
/// Python→Rust: assistant_delta / tool_call_request / memory_query_request / run_finished / run_error
/// 双向握手: hello / ready / ping / pong
pub mod msg_type {
    pub const HELLO: &str = "hello";
    pub const READY: &str = "ready";
    pub const RUN_REQUEST: &str = "run_request";
    pub const TOOL_CALL_REQUEST: &str = "tool_call_request";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const APPROVAL_RESULT: &str = "approval_result";
    pub const RUN_CANCEL: &str = "run_cancel";
    pub const RUN_FINISHED: &str = "run_finished";
    pub const RUN_ERROR: &str = "run_error";
    pub const ASSISTANT_DELTA: &str = "assistant_delta";
    pub const MEMORY_QUERY_REQUEST: &str = "memory_query_request";
    pub const DEBUG_PROMPT: &str = "debug_prompt";
    pub const DEBUG_PROMPT_RESULT: &str = "debug_prompt_result";
    pub const EMBEDDING_REQUEST: &str = "embedding_request";
    pub const EMBEDDING_RESULT: &str = "embedding_result";
    pub const PING: &str = "ping";
    pub const PONG: &str = "pong";
}
