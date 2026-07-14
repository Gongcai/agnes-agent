import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @tauri-apps/cli 注入 TAURI_DEV_HOST 时启用远程 HMR
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // Tauri 期望接管控制台输出，避免 Vite 清屏
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // 忽略 Rust 侧改动，避免前端无谓重启
      ignored: ["**/src-tauri/**"],
    },
  },
});
