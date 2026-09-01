//! PTY-backed terminal sessions used by shell tools.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, ChildKiller, MasterPty, PtySize};

use crate::error::{AppError, AppResult};
use crate::tools::sandbox::SandboxCommand;

const MAX_TERMINAL_SESSIONS: usize = 16;

#[derive(Debug, Clone)]
pub struct TerminalPoll {
    pub session_id: String,
    pub output: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTerminalInfo {
    pub session_id: String,
    pub cwd: String,
    pub rows: u16,
    pub cols: u16,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTerminalPoll {
    pub session_id: String,
    pub output: String,
    pub next_cursor: u64,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
}

#[derive(Default)]
struct TerminalState {
    output: Vec<u8>,
    base_offset: u64,
    total_offset: u64,
    delivered_offset: u64,
    truncated: bool,
    reader_closed: bool,
    exit_code: Option<i32>,
    signal: Option<String>,
}

impl TerminalState {
    fn append(&mut self, bytes: &[u8], max_output_bytes: usize) {
        self.total_offset = self.total_offset.saturating_add(bytes.len() as u64);
        self.output.extend_from_slice(bytes);
        if self.output.len() > max_output_bytes {
            let remove = self.output.len() - max_output_bytes;
            self.output.drain(..remove);
            self.base_offset = self.base_offset.saturating_add(remove as u64);
            self.truncated = true;
        }
    }

    fn take_incremental_output(&mut self) -> String {
        let mut output = String::new();
        if self.delivered_offset < self.base_offset {
            output.push_str("... earlier terminal output truncated by policy\n");
            self.delivered_offset = self.base_offset;
        }
        let start = self
            .delivered_offset
            .saturating_sub(self.base_offset)
            .min(self.output.len() as u64) as usize;
        output.push_str(&String::from_utf8_lossy(&self.output[start..]));
        self.delivered_offset = self.total_offset;
        output.replace("\r\n", "\n")
    }

    fn output_from(&self, cursor: u64) -> (String, u64) {
        let mut output = String::new();
        let start_cursor = cursor.max(self.base_offset);
        if cursor < self.base_offset {
            output.push_str("... earlier terminal output truncated by policy\n");
        }
        let start = start_cursor
            .saturating_sub(self.base_offset)
            .min(self.output.len() as u64) as usize;
        output.push_str(&String::from_utf8_lossy(&self.output[start..]));
        (output.replace("\r\n", "\n"), self.total_offset)
    }

    fn finished(&self) -> bool {
        self.exit_code.is_some() && self.reader_closed
    }
}

struct TerminalSession {
    id: String,
    owner_session_id: String,
    run_id: String,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    cwd: PathBuf,
    size: Mutex<PtySize>,
    state: Arc<Mutex<TerminalState>>,
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl TerminalSession {
    fn stop(&self) {
        self.writer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        #[cfg(unix)]
        if let Some(process_group) = self.process_group {
            terminate_process_group(process_group);
        }
        let _ = self
            .killer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .kill();
    }

    fn write(&self, chars: &str) -> AppResult<()> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(writer) = writer.as_mut() else {
            return Err(AppError::Other(format!(
                "Terminal session `{}` is no longer accepting input",
                self.id
            )));
        };
        writer.write_all(chars.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    fn resize(&self, rows: u16, cols: u16) -> AppResult<()> {
        let size = PtySize {
            rows: rows.clamp(1, 500),
            cols: cols.clamp(1, 500),
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resize(size)
            .map_err(|error| AppError::Other(format!("Unable to resize terminal: {error}")))?;
        *self.size.lock().unwrap_or_else(|error| error.into_inner()) = size;
        Ok(())
    }

    #[cfg(unix)]
    fn signal(&self, signal: nix::sys::signal::Signal) -> AppResult<()> {
        let Some(process_group) = self.process_group else {
            return Err(AppError::Other(
                "Terminal process group is unavailable".into(),
            ));
        };
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(process_group), signal)
            .map_err(|error| AppError::Other(format!("Unable to signal terminal: {error}")))
    }
}

/// Owns all terminal processes for one Agent WebSocket connection.
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
    id_prefix: String,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::with_id_prefix("agent-terminal-")
    }
}

impl TerminalManager {
    fn with_id_prefix(prefix: &str) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            id_prefix: prefix.to_string(),
        }
    }

    pub fn spawn(
        &self,
        command: SandboxCommand,
        cwd: &Path,
        owner_session_id: &str,
        run_id: &str,
        max_output_bytes: usize,
        timeout_sec: u32,
    ) -> AppResult<String> {
        self.spawn_with_size(
            command,
            cwd,
            owner_session_id,
            run_id,
            max_output_bytes,
            timeout_sec,
            PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
    }

    fn spawn_with_size(
        &self,
        command: SandboxCommand,
        cwd: &Path,
        owner_session_id: &str,
        run_id: &str,
        max_output_bytes: usize,
        timeout_sec: u32,
        size: PtySize,
    ) -> AppResult<String> {
        self.spawn_with_size_and_display_cwd(
            command,
            cwd,
            cwd,
            owner_session_id,
            run_id,
            max_output_bytes,
            timeout_sec,
            size,
        )
    }

    fn spawn_with_size_and_display_cwd(
        &self,
        command: SandboxCommand,
        cwd: &Path,
        display_cwd: &Path,
        owner_session_id: &str,
        run_id: &str,
        max_output_bytes: usize,
        timeout_sec: u32,
        size: PtySize,
    ) -> AppResult<String> {
        self.remove_finished();
        if self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
            >= MAX_TERMINAL_SESSIONS
        {
            return Err(AppError::Other(format!(
                "Too many terminal sessions are active (limit {MAX_TERMINAL_SESSIONS})"
            )));
        }

        let pty = native_pty_system();
        let pair = pty
            .openpty(size)
            .map_err(|error| AppError::Other(format!("Unable to open PTY: {error}")))?;
        let mut command = command.into_pty();
        command.cwd(cwd);
        command.env("TERM", "dumb");
        command.env("NO_COLOR", "1");
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| AppError::Other(format!("Unable to spawn PTY command: {error}")))?;
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| AppError::Other(format!("Unable to read PTY output: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| AppError::Other(format!("Unable to open PTY input: {error}")))?;
        #[cfg(unix)]
        let process_group = pair.master.process_group_leader();
        drop(pair.slave);

        let id = format!("{}{}", self.id_prefix, uuid::Uuid::new_v4());
        let state = Arc::new(Mutex::new(TerminalState::default()));
        let session = Arc::new(TerminalSession {
            id: id.clone(),
            owner_session_id: owner_session_id.to_string(),
            run_id: run_id.to_string(),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(killer),
            master: Mutex::new(pair.master),
            cwd: display_cwd.to_path_buf(),
            size: Mutex::new(size),
            state: state.clone(),
            #[cfg(unix)]
            process_group,
        });

        let read_state = state.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .append(&buffer[..read], max_output_bytes.max(1)),
                }
            }
            read_state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .reader_closed = true;
        });

        let wait_state = state;
        #[cfg(unix)]
        let wait_process_group = process_group;
        std::thread::spawn(move || {
            let result = child.wait();
            #[cfg(unix)]
            if let Some(process_group) = wait_process_group {
                terminate_process_group(process_group);
            }
            let mut state = wait_state.lock().unwrap_or_else(|error| error.into_inner());
            match result {
                Ok(status) => {
                    state.exit_code = Some(status.exit_code() as i32);
                    state.signal = status.signal().map(ToString::to_string);
                }
                Err(error) => {
                    state.exit_code = Some(-1);
                    state.signal = Some(error.to_string());
                }
            }
        });

        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.clone(), session.clone());
        if timeout_sec > 0 {
            let weak = Arc::downgrade(&session);
            std::thread::spawn(move || stop_after_timeout(weak, timeout_sec));
        }
        Ok(id)
    }

    pub async fn poll(
        &self,
        id: &str,
        owner_session_id: &str,
        yield_time: Duration,
    ) -> AppResult<TerminalPoll> {
        let session = self.session(id, owner_session_id)?;
        let deadline = Instant::now() + yield_time;
        loop {
            if session
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .finished()
                || Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let poll = snapshot(&session);
        if !poll.running {
            self.sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(id);
        }
        Ok(poll)
    }

    pub async fn write_and_poll(
        &self,
        id: &str,
        owner_session_id: &str,
        chars: &str,
        yield_time: Duration,
    ) -> AppResult<TerminalPoll> {
        let session = self.session(id, owner_session_id)?;
        if !chars.is_empty() {
            session.write(chars)?;
        }
        self.poll(id, owner_session_id, yield_time).await
    }

    pub fn stop(&self, id: &str, owner_session_id: &str) -> AppResult<TerminalPoll> {
        let session = self.session(id, owner_session_id)?;
        session.stop();
        Ok(snapshot(&session))
    }

    fn snapshot_at(
        &self,
        id: &str,
        owner_session_id: &str,
        cursor: u64,
    ) -> AppResult<UserTerminalPoll> {
        let session = self.session(id, owner_session_id)?;
        let state = session
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (output, next_cursor) = state.output_from(cursor);
        Ok(UserTerminalPoll {
            session_id: session.id.clone(),
            output,
            next_cursor,
            running: !state.finished(),
            exit_code: state.exit_code,
            signal: state.signal.clone(),
            truncated: state.truncated,
        })
    }

    fn list_infos(&self, owner_session_id: &str) -> Vec<UserTerminalInfo> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut infos = sessions
            .values()
            .filter(|session| session.owner_session_id == owner_session_id)
            .map(|session| {
                let state = session
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let size = session
                    .size
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                UserTerminalInfo {
                    session_id: session.id.clone(),
                    cwd: session.cwd.to_string_lossy().into_owned(),
                    rows: size.rows,
                    cols: size.cols,
                    running: !state.finished(),
                    exit_code: state.exit_code,
                    signal: state.signal.clone(),
                    truncated: state.truncated,
                }
            })
            .collect::<Vec<_>>();
        infos.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        infos
    }

    fn resize(&self, id: &str, owner_session_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        self.session(id, owner_session_id)?.resize(rows, cols)
    }

    #[cfg(unix)]
    fn signal(
        &self,
        id: &str,
        owner_session_id: &str,
        signal: nix::sys::signal::Signal,
    ) -> AppResult<()> {
        self.session(id, owner_session_id)?.signal(signal)
    }

    fn close(&self, id: &str, owner_session_id: &str) -> AppResult<()> {
        let session = self.session(id, owner_session_id)?;
        session.stop();
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
        Ok(())
    }

    fn stop_all(&self) {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            session.stop();
        }
    }

    pub fn stop_run(&self, run_id: &str) {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|session| session.run_id == run_id)
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            session.stop();
        }
    }

    fn session(&self, id: &str, owner_session_id: &str) -> AppResult<Arc<TerminalSession>> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let session = sessions.get(id).cloned().ok_or_else(|| {
            let mut active = sessions.keys().cloned().collect::<Vec<_>>();
            active.sort();
            let available = if active.is_empty() {
                "none".to_string()
            } else {
                active.join(", ")
            };
            AppError::Other(format!(
                "Unknown terminal session `{id}`; active sessions: {available}"
            ))
        })?;
        if session.owner_session_id != owner_session_id {
            return Err(AppError::Other(format!(
                "Terminal session `{id}` belongs to another conversation"
            )));
        }
        Ok(session)
    }

    fn remove_finished(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|_, session| {
                !session
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .finished()
            });
    }
}

/// User-visible PTYs have a separate registry and ID namespace from Agent
/// terminal sessions. They remain addressable after process exit for reconnect.
pub struct UserTerminalManager {
    terminals: TerminalManager,
}

impl Default for UserTerminalManager {
    fn default() -> Self {
        Self {
            terminals: TerminalManager::with_id_prefix("user-terminal-"),
        }
    }
}

impl UserTerminalManager {
    pub(crate) fn create(
        &self,
        command: SandboxCommand,
        host_cwd: &Path,
        guest_cwd: &Path,
        rows: u16,
        cols: u16,
        max_output_bytes: usize,
    ) -> AppResult<UserTerminalPoll> {
        let id = self.terminals.spawn_with_size_and_display_cwd(
            command,
            host_cwd,
            guest_cwd,
            "user-terminal",
            "user-terminal",
            max_output_bytes,
            0,
            PtySize {
                rows: rows.clamp(1, 500),
                cols: cols.clamp(1, 500),
                pixel_width: 0,
                pixel_height: 0,
            },
        )?;
        self.terminals.snapshot_at(&id, "user-terminal", 0)
    }

    pub(crate) fn list(&self) -> Vec<UserTerminalInfo> {
        self.terminals.list_infos("user-terminal")
    }

    pub(crate) fn poll(&self, id: &str, cursor: u64) -> AppResult<UserTerminalPoll> {
        self.terminals.snapshot_at(id, "user-terminal", cursor)
    }

    pub(crate) fn write(&self, id: &str, chars: &str) -> AppResult<()> {
        self.terminals.session(id, "user-terminal")?.write(chars)
    }

    pub(crate) fn resize(&self, id: &str, rows: u16, cols: u16) -> AppResult<()> {
        self.terminals.resize(id, "user-terminal", rows, cols)
    }

    #[cfg(unix)]
    pub(crate) fn signal_interrupt(&self, id: &str) -> AppResult<()> {
        self.terminals
            .signal(id, "user-terminal", nix::sys::signal::Signal::SIGINT)
    }

    pub(crate) fn close(&self, id: &str) -> AppResult<()> {
        self.terminals.close(id, "user-terminal")
    }

    pub(crate) fn stop_all(&self) {
        self.terminals.stop_all();
    }
}

impl Drop for UserTerminalManager {
    fn drop(&mut self) {
        self.terminals.stop_all();
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            session.stop();
        }
    }
}

fn snapshot(session: &TerminalSession) -> TerminalPoll {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let running = !state.finished();
    let output = state.take_incremental_output();
    TerminalPoll {
        session_id: session.id.clone(),
        output,
        running,
        exit_code: state.exit_code,
        signal: state.signal.clone(),
        truncated: state.truncated,
    }
}

fn stop_after_timeout(session: Weak<TerminalSession>, timeout_sec: u32) {
    std::thread::sleep(Duration::from_secs(timeout_sec as u64));
    if let Some(session) = session.upgrade() {
        if !session
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finished()
        {
            session.stop();
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: i32) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let _ = killpg(Pid::from_raw(process_group), Signal::SIGKILL);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_command(script: &str) -> SandboxCommand {
        SandboxCommand::direct(
            "/bin/bash".to_string(),
            vec!["-c".to_string(), script.to_string()],
            Vec::new(),
        )
    }

    async fn poll_until_exited(manager: &TerminalManager, id: &str, owner: &str) -> TerminalPoll {
        for _ in 0..100 {
            let poll = manager
                .poll(id, owner, Duration::from_millis(50))
                .await
                .unwrap();
            if !poll.running {
                return poll;
            }
        }
        panic!("terminal did not exit in time");
    }

    #[test]
    fn capped_output_tracks_truncation_and_incremental_reads() {
        let mut state = TerminalState::default();
        state.append(b"abcdef", 4);
        assert_eq!(
            state.take_incremental_output(),
            "... earlier terminal output truncated by policy\ncdef"
        );
        state.append(b"gh", 4);
        assert_eq!(state.take_incremental_output(), "gh");
        assert!(state.truncated);
    }

    #[tokio::test]
    async fn short_command_returns_output_and_exit_code() {
        let manager = TerminalManager::default();
        let id = manager
            .spawn(
                shell_command("printf terminal-ok"),
                Path::new("."),
                "owner",
                "run",
                1024,
                0,
            )
            .unwrap();

        let poll = poll_until_exited(&manager, &id, "owner").await;
        assert_eq!(poll.output, "terminal-ok");
        assert_eq!(poll.exit_code, Some(0));
        assert!(!poll.running);
    }

    #[tokio::test]
    async fn running_command_can_receive_input() {
        let manager = TerminalManager::default();
        let id = manager
            .spawn(
                shell_command("read value; printf 'received:%s' \"$value\""),
                Path::new("."),
                "owner",
                "run",
                1024,
                0,
            )
            .unwrap();

        let initial = manager
            .poll(&id, "owner", Duration::from_millis(50))
            .await
            .unwrap();
        assert!(initial.running);
        let sent = manager
            .write_and_poll(&id, "owner", "hello\n", Duration::from_millis(500))
            .await
            .unwrap();
        let poll = if sent.running {
            poll_until_exited(&manager, &id, "owner").await
        } else {
            sent
        };
        assert!(poll.output.contains("received:hello"));
        assert_eq!(poll.exit_code, Some(0));
    }

    #[tokio::test]
    async fn stop_terminates_running_command() {
        let manager = TerminalManager::default();
        let id = manager
            .spawn(
                shell_command("sleep 30"),
                Path::new("."),
                "owner",
                "run",
                1024,
                0,
            )
            .unwrap();

        let initial = manager
            .poll(&id, "owner", Duration::from_millis(50))
            .await
            .unwrap();
        assert!(initial.running);
        manager.stop(&id, "owner").unwrap();
        let poll = poll_until_exited(&manager, &id, "owner").await;
        assert!(!poll.running);
        assert_ne!(poll.exit_code, Some(0));
    }

    #[tokio::test]
    async fn command_output_is_bounded() {
        let manager = TerminalManager::default();
        let id = manager
            .spawn(
                shell_command("printf 1234567890"),
                Path::new("."),
                "owner",
                "run",
                4,
                0,
            )
            .unwrap();

        let poll = poll_until_exited(&manager, &id, "owner").await;
        assert!(poll.truncated);
        assert!(poll.output.contains("output truncated by policy"));
        assert!(poll.output.ends_with("7890"));
    }

    #[tokio::test]
    async fn user_terminal_keeps_cursor_output_and_separate_id_namespace() {
        let manager = UserTerminalManager::default();
        let initial = manager
            .create(
                shell_command("printf user-terminal-ok"),
                Path::new("."),
                Path::new("/root"),
                24,
                80,
                1024,
            )
            .unwrap();
        assert!(initial.session_id.starts_with("user-terminal-"));
        assert_eq!(initial.next_cursor, 0);

        let mut cursor = initial.next_cursor;
        let mut output = String::new();
        for _ in 0..20 {
            let poll = manager.poll(&initial.session_id, cursor).unwrap();
            cursor = poll.next_cursor;
            output.push_str(&poll.output);
            if !poll.running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(output.contains("user-terminal-ok"));
        assert_eq!(manager.list()[0].cwd, "/root");
        assert!(manager.resize(&initial.session_id, 40, 120).is_ok());
        manager.close(&initial.session_id).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn user_terminal_interrupt_targets_only_its_process_group() {
        let manager = UserTerminalManager::default();
        let initial = manager
            .create(
                shell_command("trap 'exit 130' INT; while true; do sleep 1; done"),
                Path::new("."),
                Path::new("/root"),
                24,
                80,
                1024,
            )
            .unwrap();
        manager.signal_interrupt(&initial.session_id).unwrap();
        for _ in 0..20 {
            let poll = manager.poll(&initial.session_id, 0).unwrap();
            if !poll.running {
                assert_ne!(poll.exit_code, Some(0));
                manager.close(&initial.session_id).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("user terminal did not stop after SIGINT");
    }
}
