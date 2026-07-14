# CODEBUDDY.md

This file provides guidance to CodeBuddy Code when working with code in this repository.

# 项目规划

- 项目总体规划与详细设计见 `ProjectPlan/` 目录，入口文档为 `ProjectPlan/PROJECT.md`（技术栈、架构、AGENTS、记忆系统、路线图等）。后续更细粒度的规划（数据库设计、记忆库设计等）也将放入该目录。

# LANGUAGE

- 自然语言使用：代码内的注释使用英文标准注释，与用户交谈、汇报或者文档使用中文

# 常用命令（项目脚手架就位后）

> 项目当前为空仓库，以下为各组件的预期命令，脚手架建立后据此核对。

Rust / Tauri 桌面端：
```bash
npm run tauri dev      # 开发模式
npm run tauri build    # 打包
cargo build            # 仅 Rust core
cargo test             # Rust 单元测试
```

Python Agent sidecar：
```bash
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
uvicorn app:app --reload        # 或作为 Tauri sidecar 启动
pytest                          # 运行测试
pytest tests/test_x.py::test_y  # 单测单个用例
```

Cloudflare Worker / D1：
```bash
wrangler dev                    # 本地开发
wrangler deploy                 # 部署
wrangler d1 execute <db> --local --file=schema.sql   # 初始化 D1
```

安卓（Tauri）：
```bash
npm run tauri android dev       # 连设备/模拟器调试
npm run tauri android build     # 打包 APK/AAB
```
