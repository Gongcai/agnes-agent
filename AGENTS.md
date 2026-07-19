# AGENTS.md

This file provides guidance to Codex when working with code in this repository.

# 项目规划

- 项目总体规划与详细设计见 `ProjectPlan/` 目录，入口文档为 `ProjectPlan/PROJECT.md`（技术栈、架构、AGENTS、记忆系统、路线图等）。后续更细粒度的规划（数据库设计、记忆库设计等）也将放入该目录。

# LANGUAGE

- 自然语言使用：代码内的注释使用英文标准注释，与用户交谈、汇报或者文档使用中文

# Git 提交

- 提交消息使用 `<type>(<scope>): <subject>` 格式，`subject` 使用中文，`type` 和 `scope` 使用英文
- 每完成一轮工作，提交代码及相关修改

# 依赖与包管理（前端）

- 本机 npm 未安装在用户目录、调用需 sudo；统一改用 pnpm 管理前端依赖（如 `pnpm add <pkg>` / `pnpm install`），已生成的 `pnpm-lock.yaml` 一并提交。

# 常用命令

Rust / Tauri 桌面端：
```bash
pnpm tauri dev         # 开发模式，sidecar 通过 uv 启动
pnpm tauri build       # 发布打包，自动冻结并内置 agentd sidecar
pnpm build:sidecar     # 仅构建并验证发布态 sidecar
cargo build            # 仅 Rust core
cargo test             # Rust 单元测试
```

Python Agent sidecar：
```bash
cd agent
uv sync
uv run python -m app.main              # 通常由 Tauri 自动启动
uv run pytest                           # 运行测试
uv run pytest tests/test_reasoning.py   # 运行指定测试文件
```

Cloudflare Worker / D1：
```bash
wrangler dev                    # 本地开发
wrangler deploy                 # 部署
wrangler d1 execute <db> --local --file=schema.sql   # 初始化 D1
```

安卓（Tauri）：
```bash
pnpm tauri android dev          # 连设备/模拟器调试
pnpm tauri android build        # 打包 APK/AAB
```
