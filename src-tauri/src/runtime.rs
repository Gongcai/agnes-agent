use std::sync::Arc;

use serde::Serialize;

use crate::agent::AgentManager;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatus {
    pub platform: String,
    pub execution_mode: String,
    pub state: String,
    pub detail: String,
}

pub trait AgentRuntime: Send + Sync {
    fn status(&self) -> AgentRuntimeStatus;
}

pub struct DesktopSidecarRuntime {
    agent: Arc<AgentManager>,
}

impl DesktopSidecarRuntime {
    pub fn new(agent: Arc<AgentManager>) -> Self {
        Self { agent }
    }
}

impl AgentRuntime for DesktopSidecarRuntime {
    fn status(&self) -> AgentRuntimeStatus {
        let running = self.agent.is_running();
        AgentRuntimeStatus {
            platform: "desktop".into(),
            execution_mode: "python-sidecar".into(),
            state: if running { "ready" } else { "stopped" }.into(),
            detail: if running {
                "桌面 Agent Runtime 已就绪".into()
            } else {
                "桌面 Agent Runtime 未启动".into()
            },
        }
    }
}

#[cfg(target_os = "android")]
pub struct AndroidLinuxRuntime;

#[cfg(target_os = "android")]
impl AndroidLinuxRuntime {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "android")]
impl AgentRuntime for AndroidLinuxRuntime {
    fn status(&self) -> AgentRuntimeStatus {
        AgentRuntimeStatus {
            platform: "android".into(),
            execution_mode: "linux-runtime".into(),
            state: "not-installed".into(),
            detail: "Android Linux Runtime 尚未安装".into(),
        }
    }
}
