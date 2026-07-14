use std::path::PathBuf;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::db::migrations;
use crate::db::repo;
use crate::error::{AppError, AppResult};

/// DB 线程处理的命令。后续工具/记忆检索在此扩展。
pub enum DbCommand {
    ListAgents {
        resp: oneshot::Sender<AppResult<Vec<repo::agents::AgentRow>>>,
    },
    InsertAgent {
        row: repo::agents::NewAgent,
        resp: oneshot::Sender<AppResult<String>>,
    },
}

/// 对外句柄：命令通过 mpsc 发给 DB 线程，结果经 oneshot 回传。
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
}

/// 在独立 OS 线程跑 rusqlite 单连接；该线程用 `blocking_recv` 消费命令。
pub fn spawn(db_path: PathBuf) -> DbActorHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<DbCommand>();
    std::thread::spawn(move || {
        let mut conn = match Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[db] 打开失败：{e}");
                return;
            }
        };
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
            }
        }
    });
    DbActorHandle { tx }
}
