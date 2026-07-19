fn main() {
    // Debug builds launch through uv and must not require a frozen release binary.
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        let target = std::env::var("TARGET").expect("missing Rust target triple");
        let extension = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        let placeholder = format!("binaries/agentd-{target}{extension}");
        if !std::path::Path::new(&placeholder).exists() {
            std::fs::create_dir_all("binaries").expect("cannot create sidecar binary directory");
            std::fs::write(&placeholder, []).expect("cannot create debug sidecar placeholder");
        }
    }
    tauri_build::build()
}
