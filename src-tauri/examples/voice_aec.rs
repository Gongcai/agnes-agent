//! Inspect or temporarily load the same PipeWire AEC used by Agnes.

#[path = "../src/error.rs"]
#[allow(dead_code)]
mod error;
#[path = "../src/voice_aec.rs"]
#[allow(dead_code)]
mod voice_aec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manager = voice_aec::VoiceAecManager::default();
    let action = std::env::args().nth(1).unwrap_or_else(|| "status".into());
    let status = match action.as_str() {
        "status" => manager.status().await,
        "routes" | "run" => manager.start().await?,
        _ => return Err("用法：voice_aec [status|routes|run]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&status)?);
    if action == "run" {
        eprintln!("临时滤镜已启用，按 Ctrl+C 释放。此命令仅检查路由，不录音。声音效果需在设备上试听。 ");
        tokio::signal::ctrl_c().await?;
    }
    manager.stop().await;
    Ok(())
}
