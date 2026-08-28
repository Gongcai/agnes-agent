# Agnes Agent

Agnes Agent is a local-first desktop application for multi-agent conversations, persistent memory, knowledge work, and personal productivity. It is closer to a character-based chat workspace than a standalone API agent: each Agent can have its own persona, system prompt, tool policy, and long-term memory.

The desktop application uses Tauri 2, React, TypeScript, Rust, SQLite, and Python sidecars. Rust owns local execution, storage, permissions, provider adapters, and synchronization. The Python sidecar runs the LangGraph/LiteLLM agent runtime.

[简体中文](README.zh-CN.md)

## Status

This repository is under active development. The desktop workflow is usable, but releases and public compatibility guarantees have not been established. Android will initially ship the lightweight client and sync foundation, then run the full local Agent through an isolated PRoot/Ubuntu Linux Runtime; SSH remains a compatibility mode for connecting to a desktop Agent.

## Features

- Multiple character-based Agents with independent sessions, prompts, tools, and memory.
- SQLite-backed Markdown memory files with per-Agent isolation, version checks, revisions, and conflict handling.
- Rust desktop tools for workspace files, shell, Git, SSH, browser access, MCP clients, and installed Skills.
- Human approval, audit records, workspace boundaries, and Linux sandboxing for sensitive operations.
- LiteLLM model routing with task-specific configuration and fallback handling.
- Local knowledge collections and RAG for text, PDF, DOCX, PPTX, and XLSX. Office parsing and optional offline PDF layout/OCR models run in dedicated sidecars.
- EPUB reading with progress, highlights, and book-specific discussions with an Agent.
- Local calendars, task lists, reminders, notifications, recurrence, and approval-aware Agent tools.
- Google Drive through the official Drive API and OAuth PKCE, including browsing, search, uploads, downloads, sharing, shared-file import, quota, and object storage workflows.
- Quark Drive through a replaceable community HTTP adapter using browser-session authentication. It supports directory browsing, file management, transfers, shared-file operations, range playback, transcoding playback, and ArtPlayer quality selection when Quark returns multiple resolutions.
- A local media proxy for range-based original-file playback when a provider has no usable transcoding stream.
- Optional Cloudflare Worker/D1 synchronization with client-side E2EE, device pairing, incremental changes, and encrypted large-object manifests.

## Architecture

```text
React / TypeScript UI
          |
          | Tauri commands and events
          v
Rust desktop core
  |       |        |
  |       |        +-- SQLite, keyring, local files, sandbox, providers
  |       +----------- media proxy, RAG, EPUB, calendar, tasks
  +------------------- Python sidecars
                         |
                         +-- LangGraph, LiteLLM, MCP, Skills

Optional synchronization:
Rust client -> Cloudflare Worker / D1 control plane -> other devices
                         |
                         +-- R2 or Google Drive encrypted object replicas
```

Storage is exposed through narrow provider ports for file sources, uploads, management, playback, quotas, shared-file import, and object storage. Provider-specific API types stay inside their adapters, keeping Google Drive, Quark Drive, R2, and future providers replaceable.

## Repository Layout

```text
src/                   React frontend and frontend tests
src-tauri/             Rust core, Tauri commands, storage, database, and sync
agent/                 Python LangGraph/LiteLLM reasoning sidecar
document-parser/       DOCX/PPTX/XLSX parser sidecar
pdf-parser/            Optional PDF model and parser package
workers/sync-api/      Cloudflare Worker and D1 synchronization service
ProjectPlan/           Architecture, security, memory, storage, and roadmap docs
scripts/               Sidecar and packaging helpers
```

## Requirements

- Node.js `22.23.2` or a compatible Node 22 release. The version is pinned in `.node-version`.
- `pnpm`.
- Rust and Cargo with the Tauri 2 toolchain.
- Python 3.12 or newer and `uv`.
- Tauri and WebKitGTK platform dependencies on Linux.

With `fnm`, select the pinned Node version and install frontend dependencies:

```bash
fnm use
corepack enable
pnpm install
```

Install Python sidecar dependencies when working on the Agent runtime:

```bash
cd agent
uv sync
cd ..
```

## Development

```bash
pnpm dev                              # Vite frontend
pnpm tauri dev                       # Tauri desktop app
pnpm build                           # TypeScript check and frontend build
pnpm test                            # Frontend tests
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

Python sidecar tests:

```bash
cd agent
uv run pytest
```

Build frozen sidecars and their smoke tests:

```bash
pnpm build:sidecar
pnpm tauri build
```

The release build freezes `agentd` and `document-parserd` with PyInstaller and embeds them as Tauri external binaries. The local script does not cross-compile Python sidecars; build them on the target platform or provide prebuilt binaries using the environment variables documented in `scripts/build-sidecar.mjs`.

## Storage Authentication

Google Drive requires a Google Desktop OAuth client JSON file. The application opens the OAuth flow and stores resulting credential material in the operating-system keyring.

Quark Drive currently uses browser-session authentication because the usable endpoints do not provide a stable public OAuth entry point for this integration. The UI accepts browser cookies and can attach additional authentication information to an existing provider account. Quark endpoints may change and are subject to rate limits and account restrictions.

Do not commit cookies, OAuth client files, access tokens, `.env` files, or other credentials. Credentials and E2EE keys are intended to stay in the OS keyring or platform keystore, not in SQLite or synchronization payloads.

## Documentation

- [Project overview and roadmap](ProjectPlan/PROJECT.md)
- [Android local Agent and Linux Runtime plan](ProjectPlan/ANDROID_RUNTIME.md)
- [Storage, RAG, and external services](ProjectPlan/STORAGE_AND_RAG.md)
- [Memory filesystem behavior](ProjectPlan/MEMORY_SYSTEM.md)
- [Memory runtime prompt](ProjectPlan/MEMORY_FILESYSTEM_PROMPT.md)
- [Skills](ProjectPlan/SKILLS.md)
- [Document parsing](ProjectPlan/DOCUMENT_PARSING.md)
- [Read With AI](ProjectPlan/READ_WITH_AI.md)
- [E2EE synchronization](ProjectPlan/E2EE.md)
- [Cloudflare sync API](workers/sync-api/README.md)

## Legal and License

Agnes Agent is licensed under the Apache License 2.0; see [LICENSE](LICENSE).

The Quark adapter communicates with undocumented service endpoints and is maintained as an independently implemented community adapter. Users are responsible for complying with relevant service terms, applicable law, and account permissions. Provider credentials should be treated as secrets, and playback or download URLs should not be shared as permanent credentials.
