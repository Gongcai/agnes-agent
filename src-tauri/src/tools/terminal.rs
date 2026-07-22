//! PTY-backed terminal sessions used by shell tools.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, ChildKiller, PtySize};

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
}

/// Owns all terminal processes for one Agent WebSocket connection.
#[derive(Default)]
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
}

impl TerminalManager {
    pub fn spawn(
        &self,
        command: SandboxCommand,
        cwd: &Path,
        owner_session_id: &str,
        run_id: &str,
        max_output_bytes: usize,
        timeout_sec: u32,
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
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
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

        let id = uuid::Uuid::new_v4().to_string();
        let state = Arc::new(Mutex::new(TerminalState::default()));
        let session = Arc::new(TerminalSession {
            id: id.clone(),
            owner_session_id: owner_session_id.to_string(),
            run_id: run_id.to_string(),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(killer),
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
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Other(format!("Unknown terminal session `{id}`")))?;
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
}
