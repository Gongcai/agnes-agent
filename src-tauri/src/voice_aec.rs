use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Serialize)]
pub struct VoiceAudioDevice {
    pub name: String,
    pub description: String,
}

#[derive(Default, Serialize)]
pub struct VoiceAecStatus {
    pub supported: bool,
    pub running: bool,
    pub routes_ready: bool,
    pub needs_reconnect: bool,
    pub source_name: Option<String>,
    pub source_label: Option<String>,
    pub input: Option<VoiceAudioDevice>,
    pub output: Option<VoiceAudioDevice>,
    pub message: Option<String>,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::process::Stdio;
    use std::time::Duration;

    use serde_json::Value;
    use tokio::process::Command;
    use tokio::sync::Mutex;

    use super::*;

    const AEC_SOURCE_NAME: &str = "agnes.aec.source";
    const AEC_SINK_NAME: &str = "agnes.aec.sink";
    const AEC_CAPTURE_NAME: &str = "echo-cancel-capture";
    const AEC_PLAYBACK_NAME: &str = "echo-cancel-playback";

    struct RunningAec {
        module_id: u32,
        label: String,
        input: VoiceAudioDevice,
        output: VoiceAudioDevice,
        previous_default_source: String,
        previous_default_sink: String,
    }

    #[derive(Default)]
    pub struct VoiceAecManager {
        running: Mutex<Option<RunningAec>>,
    }

    async fn snapshot() -> AppResult<Vec<Value>> {
        let mut command = Command::new("pw-dump");
        command.kill_on_drop(true).stderr(Stdio::null());
        let output = tokio::time::timeout(Duration::from_secs(2), command.output())
            .await
            .map_err(|_| AppError::Other("读取音频路由超时，请检查 PipeWire 服务。".into()))?
            .map_err(|_| AppError::Other("无法读取音频路由，请确认已安装 PipeWire 的 pw-dump 工具。".into()))?;
        if !output.status.success() {
            return Err(AppError::Other("无法连接 PipeWire，请检查本机音频服务。".into()));
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn node<'a>(objects: &'a [Value], name: &str) -> Option<&'a Value> {
        objects.iter().find(|object| {
            object["type"] == "PipeWire:Interface:Node"
                && object["info"]["props"]["node.name"] == name
        })
    }

    fn physical_device(objects: &[Value], kind: &str, name: &str) -> Option<VoiceAudioDevice> {
        let props = &node(objects, name)?["info"]["props"];
        let expected_class = if kind == "source" { "Audio/Source" } else { "Audio/Sink" };
        if props["media.class"] != expected_class
            || props["node.virtual"] == true || props["node.virtual"] == "true"
            || props["device.id"].is_null()
        {
            return None;
        }
        Some(VoiceAudioDevice {
            name: name.into(),
            description: props["node.description"].as_str().unwrap_or(name).into(),
        })
    }

    async fn pactl_value(argument: &str) -> AppResult<String> {
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            Command::new("pactl").arg(argument).output(),
        )
        .await
        .map_err(|_| AppError::Other("读取 PulseAudio 默认设备超时，请检查 PipeWire 服务。".into()))?
        .map_err(|_| AppError::Other("无法执行 pactl，请确认已安装 PipeWire PulseAudio 兼容层。".into()))?;
        if !output.status.success() {
            return Err(AppError::Other(format!(
                "pactl {argument} 执行失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            return Err(AppError::Other(format!("pactl {argument} 未返回默认设备。")));
        }
        Ok(value)
    }

    async fn current_physical_device(objects: &[Value], kind: &str) -> AppResult<VoiceAudioDevice> {
        let argument = if kind == "source" {
            "get-default-source"
        } else {
            "get-default-sink"
        };
        let name = pactl_value(argument).await?;
        physical_device(objects, kind, &name).ok_or_else(|| {
            AppError::Other(format!(
                "当前默认 {kind} 不是可用的物理 PipeWire 设备：{name}"
            ))
        })
    }

    async fn set_default(kind: &str, name: &str) -> AppResult<()> {
        let argument = if kind == "source" {
            "set-default-source"
        } else {
            "set-default-sink"
        };
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            Command::new("pactl").args([argument, name]).output(),
        )
        .await
        .map_err(|_| AppError::Other("切换默认音频设备超时，请检查 PipeWire 服务。".into()))?
        .map_err(|_| AppError::Other("无法执行 pactl，请确认已安装 PipeWire PulseAudio 兼容层。".into()))?;
        if !output.status.success() {
            return Err(AppError::Other(format!(
                "切换默认 {kind} 失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    struct StaleAecModule {
        id: String,
        source_name: Option<String>,
        sink_name: Option<String>,
        source_master: Option<String>,
        sink_master: Option<String>,
    }

    fn module_argument(args: &str, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        args.split_whitespace()
            .find_map(|field| field.strip_prefix(&prefix))
            .map(|value| value.trim_matches(['\'', '"']).to_string())
    }

    fn is_aec_endpoint(name: Option<&str>, endpoint: &str) -> bool {
        name.is_some_and(|name| {
            name == endpoint
                || name
                    .strip_prefix(endpoint)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    }

    async fn stale_modules() -> Vec<StaleAecModule> {
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            Command::new("pactl").args(["list", "modules", "short"]).output(),
        ).await;
        let Ok(Ok(output)) = output else { return Vec::new() };
        if !output.status.success() { return Vec::new(); }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, char::is_whitespace).filter(|field| !field.is_empty());
                let id = fields.next()?.to_string();
                let module = fields.next()?;
                let args = fields.next().unwrap_or_default();
                if module != "module-echo-cancel"
                    || !(args.contains("source_name=agnes.aec.source")
                        || args.contains("sink_name=agnes.aec.sink")
                        || args.contains("Agnes Echo Cancelled Microphone"))
                {
                    return None;
                }
                Some(StaleAecModule {
                    id,
                    source_name: module_argument(args, "source_name"),
                    sink_name: module_argument(args, "sink_name"),
                    source_master: module_argument(args, "source_master"),
                    sink_master: module_argument(args, "sink_master"),
                })
            })
            .collect()
    }

    async fn cleanup_stale_modules() {
        let modules = stale_modules().await;
        if modules.is_empty() {
            return;
        }

        // An interrupted process can leave the PulseAudio defaults pointing at
        // a virtual endpoint. Restore the physical masters before removing it.
        let current_source = pactl_value("get-default-source").await.ok();
        let current_sink = pactl_value("get-default-sink").await.ok();
        let source_master = current_source.as_deref().and_then(|current| {
            modules.iter().find_map(|module| {
                is_aec_endpoint(module.source_name.as_deref(), AEC_SOURCE_NAME)
                    .then_some(module.source_master.as_deref())
                    .flatten()
                    .filter(|_| is_aec_endpoint(Some(current), AEC_SOURCE_NAME))
            })
        });
        let sink_master = current_sink.as_deref().and_then(|current| {
            modules.iter().find_map(|module| {
                is_aec_endpoint(module.sink_name.as_deref(), AEC_SINK_NAME)
                    .then_some(module.sink_master.as_deref())
                    .flatten()
                    .filter(|_| is_aec_endpoint(Some(current), AEC_SINK_NAME))
            })
        });
        if let Some(name) = source_master {
            let _ = set_default("source", name).await;
        }
        if let Some(name) = sink_master {
            let _ = set_default("sink", name).await;
        }

        for module in modules {
            let _ = Command::new("pactl")
                .args(["unload-module", &module.id])
                .output()
                .await;
        }
    }

    fn connected(objects: &[Value], from: &str, to: &str) -> bool {
        let Some(from_id) = node(objects, from).and_then(|node| node["id"].as_u64()) else { return false };
        let Some(to_id) = node(objects, to).and_then(|node| node["id"].as_u64()) else { return false };
        let input_ports: Vec<_> = objects.iter().filter(|object| {
            object["type"] == "PipeWire:Interface:Port"
                && object["info"]["props"]["node.id"].as_u64() == Some(to_id)
                && object["info"]["direction"] == "input"
        }).collect();
        let links: Vec<_> = objects.iter().filter(|object| {
            object["type"] == "PipeWire:Interface:Link"
                && object["info"]["output-node-id"].as_u64() == Some(from_id)
                && object["info"]["input-node-id"].as_u64() == Some(to_id)
        }).collect();
        // A physical sink can have unrelated application streams. Only require
        // every input port of the AEC endpoint to have a healthy matching link.
        !input_ports.is_empty()
            && input_ports.iter().all(|port| links.iter().any(|link| {
                link["info"]["state"] != "error"
                    && link["info"]["input-port-id"] == port["id"]
            }))
    }

    async fn running_status(aec: &RunningAec, objects: &[Value]) -> VoiceAecStatus {
        let current_source = pactl_value("get-default-source").await.ok();
        let current_sink = pactl_value("get-default-sink").await.ok();
        let needs_reconnect = current_source.as_deref() != Some(AEC_SOURCE_NAME)
            || current_sink.as_deref() != Some(AEC_SINK_NAME);
        let routes_ready = !needs_reconnect
            && node(objects, AEC_SOURCE_NAME).is_some()
            && node(objects, AEC_SINK_NAME).is_some()
            && connected(objects, &aec.input.name, AEC_CAPTURE_NAME)
            && connected(objects, AEC_PLAYBACK_NAME, &aec.output.name);
        VoiceAecStatus {
            supported: true,
            running: true,
            routes_ready,
            needs_reconnect,
            source_name: Some(AEC_SOURCE_NAME.into()),
            source_label: Some(aec.label.clone()),
            input: Some(aec.input.clone()),
            output: Some(aec.output.clone()),
            message: if needs_reconnect {
                Some("回声消除的默认输入或输出已被切换，请重新连接回声消除。".into())
            } else if !routes_ready {
                Some("麦克风或播放参考未连接，请重新连接回声消除。".into())
            } else { None },
        }
    }

    async fn stop_child(running: &mut Option<RunningAec>) {
        if let Some(aec) = running.take() {
            if pactl_value("get-default-source").await.ok().as_deref() == Some(AEC_SOURCE_NAME) {
                let _ = set_default("source", &aec.previous_default_source).await;
            }
            if pactl_value("get-default-sink").await.ok().as_deref() == Some(AEC_SINK_NAME) {
                let _ = set_default("sink", &aec.previous_default_sink).await;
            }
            let _ = Command::new("pactl")
                .args(["unload-module", &aec.module_id.to_string()])
                .output()
                .await;
        }
        // A previous Agnes process may have exited before its in-memory module
        // handle was recorded. Fixed names let us clean those instances too.
        cleanup_stale_modules().await;
    }

    impl VoiceAecManager {
        pub async fn cleanup_stale(&self) {
            let mut running = self.running.lock().await;
            stop_child(&mut running).await;
        }

        pub async fn status(&self) -> VoiceAecStatus {
            let running = self.running.lock().await;
            match snapshot().await {
                Ok(objects) => match running.as_ref() {
                    Some(aec) => running_status(aec, &objects).await,
                    None => {
                        let input = current_physical_device(&objects, "source").await.ok();
                        let output = current_physical_device(&objects, "sink").await.ok();
                        VoiceAecStatus { supported: true, input, output, ..Default::default() }
                    }
                },
                Err(error) => VoiceAecStatus {
                    supported: true,
                    running: running.is_some(),
                    message: Some(error.to_string()),
                    ..Default::default()
                },
            }
        }

        pub async fn start(&self) -> AppResult<VoiceAecStatus> {
            let mut running = self.running.lock().await;
            // Reconnect starts from a clean module and restores the previous
            // defaults before selecting the current physical devices.
            stop_child(&mut running).await;
            let objects = snapshot().await?;
            let previous_default_source = pactl_value("get-default-source").await?;
            let previous_default_sink = pactl_value("get-default-sink").await?;
            let input = physical_device(&objects, "source", &previous_default_source).ok_or_else(|| AppError::Other(
                "请先在系统音频设置中选择物理麦克风作为默认输入，再启用回声消除。".into()
            ))?;
            let output = physical_device(&objects, "sink", &previous_default_sink).ok_or_else(|| AppError::Other(
                "请先在系统音频设置中选择扬声器或耳机作为默认输出，再启用回声消除。".into()
            ))?;
            let instance = uuid::Uuid::new_v4().simple().to_string();
            let label = format!("Agnes Echo Cancelled Microphone ({})", &instance[..6]);
            let source_properties = format!("device.description='{label}' device.virtual=true");
            let source_properties_arg = format!("source_properties={source_properties}");
            let sink_properties_arg = "sink_properties=device.description='Agnes Echo Reference' device.virtual=true";
            let source_master_arg = format!("source_master={}", input.name);
            let sink_master_arg = format!("sink_master={}", output.name);
            let mut command = Command::new("pactl");
            command.args([
                "load-module", "module-echo-cancel",
                &format!("source_name={AEC_SOURCE_NAME}"),
                &format!("sink_name={AEC_SINK_NAME}"),
                &source_properties_arg,
                sink_properties_arg,
                &source_master_arg,
                &sink_master_arg,
                "aec_method=webrtc", "rate=48000", "channels=1",
            ])
                .stdin(Stdio::null()).stderr(Stdio::null())
                .kill_on_drop(true);
            let output_result = command.output().await.map_err(|_| AppError::Other(
                "无法启动回声消除，请确认已安装 PipeWire PulseAudio 兼容层。".into()
            ))?;
            if !output_result.status.success() {
                return Err(AppError::Other("PipeWire 回声消除模块加载失败，请检查 PulseAudio 兼容服务。".into()));
            }
            let module_id: u32 = String::from_utf8_lossy(&output_result.stdout).trim().parse()
                .map_err(|_| AppError::Other("回声消除模块未返回有效句柄。".into()))?;
            if let Err(error) = set_default("source", AEC_SOURCE_NAME).await {
                let _ = Command::new("pactl")
                    .args(["unload-module", &module_id.to_string()])
                    .output()
                    .await;
                return Err(error);
            }
            if let Err(error) = set_default("sink", AEC_SINK_NAME).await {
                let _ = set_default("source", &previous_default_source).await;
                let _ = set_default("sink", &previous_default_sink).await;
                let _ = Command::new("pactl")
                    .args(["unload-module", &module_id.to_string()])
                    .output()
                    .await;
                return Err(error);
            }
            *running = Some(RunningAec {
                module_id,
                label,
                input,
                output,
                previous_default_source,
                previous_default_sink,
            });
            let connecting = async {
                for _ in 0..20 {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    let aec = running.as_mut().expect("AEC start owns its lock");
                    let status = running_status(aec, &snapshot().await?).await;
                    if status.routes_ready { return Ok(status); }
                    if status.needs_reconnect { break; }
                }
                Err(AppError::Other(
                    "回声消除未能连接麦克风和输出参考，请检查设备与 PipeWire WebRTC AEC 组件后重试。".into()
                ))
            };
            let result = tokio::time::timeout(Duration::from_secs(6), connecting).await
                .unwrap_or_else(|_| Err(AppError::Other("回声消除连接超时，请检查音频设备后重试。".into())));
            if result.is_err() { stop_child(&mut running).await; }
            result
        }

        pub async fn stop(&self) {
            stop_child(&mut *self.running.lock().await).await;
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::VoiceAecManager;

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct VoiceAecManager;

#[cfg(not(target_os = "linux"))]
impl VoiceAecManager {
    pub async fn status(&self) -> VoiceAecStatus { VoiceAecStatus::default() }
    pub async fn cleanup_stale(&self) {}
    pub async fn start(&self) -> AppResult<VoiceAecStatus> {
        Err(AppError::Other("PipeWire 回声消除仅支持 Linux 桌面端。".into()))
    }
    pub async fn stop(&self) {}
}

#[tauri::command]
pub async fn get_voice_aec_status(state: tauri::State<'_, VoiceAecManager>) -> AppResult<VoiceAecStatus> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn start_voice_aec(
    app: tauri::AppHandle,
    state: tauri::State<'_, VoiceAecManager>,
) -> AppResult<VoiceAecStatus> {
    use tauri::Emitter;
    let result = state.start().await;
    // A reconnect can remove the old source even if starting the new one fails.
    let _ = app.emit("voice://aec_changed", ());
    result
}

#[tauri::command]
pub async fn stop_voice_aec(app: tauri::AppHandle, state: tauri::State<'_, VoiceAecManager>) -> AppResult<()> {
    use tauri::Emitter;
    state.stop().await;
    let _ = app.emit("voice://aec_changed", ());
    Ok(())
}
