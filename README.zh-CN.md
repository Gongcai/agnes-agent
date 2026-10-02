<p align="center"><img src="public/logo.png" width="96" height="96" alt="Agnes Agent" /></p>

# Agnes Agent

Agnes Agent 是一个本地优先的桌面应用，用于多 Agent 对话、持久记忆、知识库和个人效率管理。它更接近带角色卡的聊天工作区，而不是单纯的 API Agent：每个 Agent 都可以拥有独立的人设、系统提示词、工具策略和长期记忆。

桌面端使用 Tauri 2、React、TypeScript、Rust、SQLite 和 Python sidecar 构建。Rust 核心负责本地执行、存储、权限、Provider adapter 和同步；Python sidecar 负责 LangGraph/LiteLLM Agent 运行时。

[English](README.md)

## 项目状态

项目仍在积极开发中。桌面端主要工作流已经可用，但暂未建立正式发布版本和公共兼容性承诺。Android 端先交付轻客户端和同步基础，后续通过独立的 PRoot/Ubuntu Linux Runtime 在手机本地运行完整 Agent；SSH 作为连接桌面 Agent 的兼容模式保留。

## 当前能力

- 多角色 Agent：每个 Agent 拥有独立会话、提示词、工具权限和记忆。
- 基于 SQLite 的 Markdown 虚拟记忆文件系统：按 Agent 隔离，支持版本校验、修订和冲突处理。
- Rust 桌面执行核心：支持工作区文件、Shell、Git、SSH、浏览器、MCP Client 和已安装 Skills。
- 敏感操作人工审批、审计记录、工作区边界和 Linux 沙箱。
- 通过 LiteLLM 进行模型路由，支持按任务配置模型和故障回退。
- 本地知识库和 RAG：支持文本、PDF、DOCX、PPTX、XLSX 导入。Office 结构化解析和可选的离线 PDF 版面/OCR 模型在独立 sidecar 中运行。
- EPUB 阅读：支持阅读进度、划线，以及围绕书籍与指定 Agent 的连续讨论。
- 本地日历、待办、提醒、通知、重复事件和受审批约束的 Agent 工具。
- Google Drive：使用官方 Drive API 和 OAuth PKCE，支持目录浏览、搜索、上传、下载、分享、转存分享文件、配额和对象存储流程。
- 夸克网盘：使用可替换的 community HTTP adapter 和浏览器 Cookie 鉴权，支持目录浏览、文件管理、传输、分享文件操作、Range 播放、转码播放，以及在夸克返回多个清晰度时使用 ArtPlayer 选择清晰度。
- 本地媒体代理：当网盘没有可用转码流时，为原文件播放提供 Range 请求代理。
- 可选的 Cloudflare Worker/D1 同步：客户端 E2EE、设备配对、增量变更和加密大对象清单。

## 架构

```text
React / TypeScript 前端
          |
          | Tauri commands 和 events
          v
Rust 桌面核心
  |       |        |
  |       |        +-- SQLite、Keyring、本地文件、沙箱、网盘 Provider
  |       +----------- 媒体代理、RAG、EPUB、日历、待办
  +------------------- Python sidecar
                         |
                         +-- LangGraph、LiteLLM、MCP、Skills

可选同步：
Rust 客户端 -> Cloudflare Worker / D1 控制面 -> 其他设备
                         |
                         +-- R2 或 Google Drive 加密对象副本
```

应用使用文件源、上传、文件管理、播放、配额、分享文件转存和对象存储等窄化 Provider 端口，使 Google Drive、夸克网盘、R2 以及未来的其他 Provider 可以在服务边界替换。

## 目录结构

```text
src/                   React 前端和前端测试
src-tauri/             Rust 核心、Tauri commands、存储、数据库和同步
agent/                 Python LangGraph/LiteLLM 推理 sidecar
document-parser/       DOCX/PPTX/XLSX 解析 sidecar
pdf-parser/            可选 PDF 模型和解析包
workers/sync-api/      Cloudflare Worker 和 D1 同步服务
ProjectPlan/           架构、安全、记忆、存储和路线图文档
scripts/               sidecar 和打包脚本
```

## 开发环境

桌面端开发需要：

- Node.js `22.23.2` 或兼容的 Node 22 版本，仓库通过 `.node-version` 固定版本。
- `pnpm`。
- Rust、Cargo 和 Tauri 2 工具链。
- Python 3.12 或更高版本，以及 `uv`。
- Tauri 和 Linux WebKitGTK 所需的平台依赖。

使用 `fnm` 时，可以自动选择仓库固定的 Node 版本并安装前端依赖：

```bash
fnm use
corepack enable
pnpm install
```

开发 Agent 运行时前，安装 Python sidecar 依赖：

```bash
cd agent
uv sync
cd ..
```

## 开发和构建

```bash
pnpm dev                              # Vite 前端
pnpm tauri dev                       # Tauri 桌面应用
pnpm build                           # TypeScript 检查和前端构建
pnpm test                            # 前端测试
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

Python sidecar 测试：

```bash
cd agent
uv run pytest
```

构建冻结后的 sidecar 并运行 smoke test：

```bash
pnpm build:sidecar
pnpm tauri build
```

发布构建会使用 PyInstaller 冻结 `agentd` 和 `document-parserd`，并将其作为 Tauri external binaries 内置。当前构建脚本不支持 Python sidecar 交叉编译；请在目标平台构建，或按照 `scripts/build-sidecar.mjs` 中的约定提供预编译二进制文件。

## 网盘鉴权

Google Drive 需要 Google Desktop OAuth client JSON 文件。应用会打开 OAuth 授权流程，并将最终的凭据材料保存到操作系统 Keyring。

夸克网盘目前使用浏览器会话鉴权，因为本集成所需的可用接口没有稳定的公开 OAuth 入口。界面支持输入浏览器 Cookies，也支持为已有网盘账户补充额外鉴权信息。夸克接口可能受到上游变更、限流和账户限制影响。

不要提交 Cookies、OAuth client 文件、访问令牌、`.env` 文件或其他凭据。网盘凭据和 E2EE 主密钥应保存在操作系统 Keyring 或平台 Keystore 中，不应写入 SQLite 或同步 payload。

## 文档

- [项目总览和路线图](ProjectPlan/PROJECT.md)
- [Android 本地 Agent 与 Linux Runtime 规划](ProjectPlan/ANDROID_RUNTIME.md)
- [存储、RAG 和外部服务](ProjectPlan/STORAGE_AND_RAG.md)
- [记忆文件系统](ProjectPlan/MEMORY_SYSTEM.md)
- [记忆运行时提示词](ProjectPlan/MEMORY_FILESYSTEM_PROMPT.md)
- [Skills 系统](ProjectPlan/SKILLS.md)
- [文档解析](ProjectPlan/DOCUMENT_PARSING.md)
- [Read With AI](ProjectPlan/READ_WITH_AI.md)
- [E2EE 同步](ProjectPlan/E2EE.md)
- [Cloudflare 同步 API](workers/sync-api/README.md)

## 法律和许可证

Agnes Agent 使用 Apache License 2.0，详见 [LICENSE](LICENSE)。

夸克 adapter 使用未公开的服务接口，并作为独立实现的 community adapter 维护。用户需要自行遵守相关服务条款、适用法律和账户权限。Provider 凭据应当作为敏感信息处理，播放或下载 URL 不应被当作可以长期分享的凭据。
