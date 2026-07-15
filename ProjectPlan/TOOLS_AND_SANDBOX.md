# 基础工具调用 + 安全沙箱 设计

> 目标：补齐基础工具集、把 workspace 打通为工具默认 cwd、引入分层安全沙箱、统一风险/审批模型。
> 范围：Linux 桌面优先（项目当前运行环境），macOS/Android 后续按需适配。

## 一、当前缺口（已勘探确认）

1. 沙箱=零：仅路径白名单 + `env_clear`，shell 可在 allowed_cwd 内任意 `>` 写、`curl` 外联、`sh` 嵌套。
2. `risk_level` 与审批解耦：`determine_risk` 子串匹配（`"rm "`/`"sudo"`）易绕过，且 ws_server 审判只读 `policy.*.approval` 字符串。
3. `ShellPolicy.deny_write_outside_workspace` 字段存在但 executor 从不读。
4. git：未 `env_clear`、无子命令限制、超时硬编码 30s，LLM 可跑 `git config --global` / `git clean -fdx`。
5. workspace cwd 未打通：executor 默认 cwd `"."`（=Tauri 进程 cwd），从不查 `session.workspace_id → workspaces.folder_path`。
6. 工具集不全：仅 shell/file_read/file_write/git；缺 file_edit、只读搜索（glob/grep）、list。
7. approval 语义三工具不一致（`always|never` / `write|never` / `push|never`）。

## 二、设计目标与原则

- **默认安全**：workspace 之外只读、网络按策略控制（默认开）、危险动作默认审批。
- **分层防御**：路径白名单（已有）+ Landlock FS 隔离（新增）+ 资源限额 + 超时 + 输出截断 + 审批。
- **workspace 为中心**：工具默认 cwd = workspace.folder_path；FS 写范围默认限定 workspace。
- **风险驱动审批**：统一 tier `never | on-write | on-risk | always`，risk_level 真正驱动审批。
- **最小破坏**：现有 4 工具签名不变，新增工具与沙箱作为增量。

## 三、工具集补全

保留：`shell` / `file_read` / `file_write` / `git`。
新增（减少对 raw shell 的依赖，读操作更安全）：

| 工具 | 用途 | 风险 |
|---|---|---|
| `file_edit` | 字符串精确替换（str_replace，Edit 工具风格），避免整文件重写 | Medium |
| `list_files` | 列目录（glob 模式），只读 | Low |
| `grep` | 递归内容搜索（只读，ripgrep 风式） | Low |
| `apply_patch` | 统一 diff 补丁应用（codex 风格，多段一次性改） | Medium |

> `memory_search` / `browser` / `ssh` 暂不实现（超出「基础」范围，后续单独设计）。

### 工具模块拆分
`src-tauri/src/tools/` 改为：
```
tools/
├── mod.rs            # ToolExecutor 路由 + trait
├── policy.rs         # ToolPolicy（重构，见第五节）
├── sandbox.rs        # Landlock + 资源限额（新增）
├── workspace.rs      # session→workspace cwd 解析（新增）
└── builtin/
    ├── mod.rs        # BuiltinTool trait
    ├── shell.rs
    ├── file_read.rs
    ├── file_write.rs
    ├── file_edit.rs
    ├── git.rs
    ├── list_files.rs
    ├── grep.rs
    └── apply_patch.rs
```
统一 trait：
```rust
#[async_trait]
trait BuiltinTool {
    fn name(&self) -> &'static str;
    fn risk(&self, args: &Value) -> Risk;          // Low|Medium|High
    fn schema(&self) -> Value;                       // 供 Python 声明给 LLM
    async fn execute(&self, ctx: &ToolCtx) -> AppResult<ToolResult>;
}
pub struct ToolCtx<'a> {
    pub db: &'a DbActorHandle,
    pub session_id: &'a str,
    pub tool_call_id: &'a str,
    pub args: &'a Value,
    pub policy: &'a ToolPolicy,
    pub workspace_cwd: Option<PathBuf>,  // 解析后的 workspace folder_path
    pub sandbox: &'a dyn SandboxGuard,   // 路径能力检查 + 子进程沙箱构造器
}
```
ToolExecutor `execute` 统一：审计入志 → 查 workspace cwd → 应用沙箱 → `dispatch(tool).execute(ctx)` → 回填。

## 四、workspace cwd 打通（缺口 #5）

`ToolExecutor` 构造时持有 `db`，每次 `execute`：
1. `db.get_session(session_id)` → `session.workspace_id`。
2. 若有：`db.get_workspace(workspace_id)` → `folder_path`，作为默认 cwd。
3. 工具 args.cwd 非空则覆盖（但仍经沙箱/白名单校验）。
4. 无 workspace：回退到 `policy.shell.allowed_cwd[0]`（保持旧行为）。

Python 侧 `get_available_tools` 的 `cwd` 描述改为真实生效。ContextSnapshot 已透传 session，无需改协议。

## 五、统一风险/审批模型（缺口 #2、#7）

### Risk 枚举
```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Risk { Low, Medium, High }
```
每个工具 `fn risk(&self, args) -> Risk`：
- `shell`：解析命令首 token + 子串启发（`rm`/`rmdir`/`mv` 跨目录/`sudo`/`>`/`curl`/`wget`/`chmod 777`/`dd` → High；其余 Medium）。仍非完备，但配合沙箱降低风险。
- `file_write`/`file_edit`：Medium；写 workspace 外 → High。
- `file_read`/`list_files`/`grep`：Low；读敏感路径（`~/.ssh`、`/etc`）→ Medium。
- `git`：`push`/`reset --hard`/`clean -fdx` → High；其余 Low。

### 统一 approval tier
ToolPolicy 改为每工具一个 `approval: ApprovalTier`：
```rust
pub enum ApprovalTier { Never, OnWrite, OnRisk, Always }
```
- `Never`：直接执行。
- `OnWrite`：写操作（shell 含 `>`/写工具/git push）才审批。
- `OnRisk`：risk >= High 才审批。
- `Always`：每次审批。

`needs_approval(tool, args, tier, risk)`：
```
match tier {
  Never => false,
  OnWrite => is_write_op(tool, args),
  OnRisk => risk == High,
  Always => true,
}
```
默认：`shell=OnRisk`、`file_write=OnWrite`、`file_edit=OnWrite`、`git=OnRisk`、`file_read/list_files/grep=Never`。
迁移：旧 JSON `approval:"always"|"write"|"push"|"never"` 自动映射到新 tier（一次性迁移函数）。

## 六、安全沙箱（缺口 #1、#3、#4）

### 6.1 Landlock（主防线，Linux 5.13+，无外部二进制）
crate `landlock`。对每个工具子进程（shell/git）与 file 工具操作应用 FS 访问规则：
- **workspace 目录**：读 + 写 + 执行（遍历）。
- **只读根目录**（`policy.file.allowed_roots` 除 workspace）：只读。
- **系统目录** `/usr` `/bin` `/lib*` `/etc`（必要部分）：只读。
- **临时目录** `$TMPDIR`（或 `/tmp`）：读写（部分工具需要）。
- **其它路径**：默认拒绝（Landlock 的 deny-by-default）。

`shell`/`git` 通过当前桌面二进制的内部 helper 启动：helper 在单线程进程中应用 Landlock/rlimit 后 `exec` 目标程序，避免在 Tauri 多线程主进程的 `pre_exec` 阶段执行复杂逻辑。原生文件工具不把不可逆的 Landlock 规则施加到 Tauri 主线程，而是统一使用 symlink-aware 的规范化路径与 `SandboxGuard` 读写能力检查。

Landlock 不可用（老内核/非 Linux）→ 降级为路径白名单（现状）+ 日志告警，不阻断。

### 6.2 网络隔离（默认开，可关）
`ToolPolicy` 加：
```rust
pub struct NetworkPolicy { pub allow: bool }  // 默认 true（默认开）
```
- `allow=true`（默认）：放行网络，不作额外拦截（贴合用户习惯）。
- `allow=false`（用户在角色工具策略里关闭）：shell/git 子进程用 **bubblewrap**（若可用）`--unshare-net` 隔离网络；bwrap 不可用则检测 `curl`/`wget`/`ssh`/`nc`/`git push` 等网络动作拒绝。
- `git push` 始终受 `git.approval` tier 约束（与网络策略独立）。

> bwrap 作为**可选增强**（更强隔离：只读根挂载 + workspace/tmp 可写绑定 + PID namespace）；存在且探测可用时自动启用。`allow=false` 时额外创建 net namespace；不存在则降级到 Landlock + 启发式网络拦截。不作为硬依赖。

### 6.3 资源限额（setrlimit，crate `nix`）
对 shell/git 子进程设：
- `RLIMIT_CPU`（CPU 秒，默认 60s）
- `RLIMIT_AS`（虚拟内存，默认 1GB）
- `RLIMIT_FSIZE`（单文件写大小，默认 50MB）
- `RLIMIT_NPROC`（子进程数，默认 64）

### 6.4 git 加固（缺口 #4）
- `env_clear` + `env_allowlist`（与 shell 一致）。
- 子命令黑名单：`config --global`/`config --system`/`clean -xfd`/`reset --hard`/`filter-branch` → 拒绝或升 High+审批。
- 超时读 `policy.git.timeout_sec`（新增字段，默认 30）。
- cwd 必须在 workspace 或 allowed_cwd 内。

### 6.5 shell 写出范围（缺口 #3）
`deny_write_outside_workspace=true` 时：
- Landlock 已限定 workspace 外不可写（主防线）。
- 额外检测 `>`/`>>`/`tee` 目标路径，workspace 外拒绝。

### 6.6 Windows 沙箱策略（跨平台考量）
Landlock 是 Linux 专属（内核 LSM）。Windows 采用分层降级方案：

1. **主防线（后续增强）—— Job Object + Restricted Token**：
   - 用 `windows-rs` 创建 Job Object，设 `JOB_OBJECT_LIMIT_BREAKAWAY_OK` + UI/桌面限制 + 进程数/内存限额。
   - 用 `CreateRestrictedToken` 去掉危险权限组（Administrators），降权运行子进程。
   - 子进程默认在低完整性级别（Low Integrity）运行，配合 ACL 限制文件写。
2. **路径 ACL 收紧**：对 workspace 外的敏感目录显式 DENY 写权限（通过 `icacls` 或直接 ACL API）。
3. **当前降级方案（与 Linux Landlock 不可用时一致）**：路径白名单 + `env_clear` + 资源限额 + 超时 + 输出截断 + 审批。Windows 暂不依赖 Job Object（作为 Phase F 增强项）。

> 抽象出 `trait SandboxGuard`，Linux 实现 = Landlock+rlimits，Windows 实现 = 路径白名单（+ 未来 Job Object）。工具层只面向 trait，不感知平台。

## 七、ToolPolicy 重构后结构（汇总）

```rust
pub struct ToolPolicy {
    pub shell: ShellPolicy,
    pub file: FilePolicy,
    pub git: GitPolicy,
    pub network: NetworkPolicy,
    pub sandbox: SandboxPolicy,
}
pub struct ShellPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub allowed_cwd: Vec<String>,
    pub deny_write_outside_workspace: bool,
    pub timeout_sec: u32,
    pub max_output_bytes: u32,
    pub env_allowlist: Vec<String>,
}
pub struct FilePolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub allowed_roots: Vec<String>,
}
pub struct GitPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub timeout_sec: u32,            // 新增
}
pub struct SandboxPolicy {
    pub landlock: bool,
    pub bwrap: BwrapMode,             // Auto | Disabled | Required
    pub rlimits: bool,
    pub cpu_time_sec: u64,
    pub memory_bytes: u64,
    pub file_size_bytes: u64,
    pub max_processes: u64,
}
pub struct NetworkPolicy { pub allow: bool }
```

`file_edit`/`apply_patch` 复用 `file.approval`（默认 `OnWrite`）；`list_files`/`grep` 固定为只读免审批。这样 UI 仍保持 Shell / 文件 / Git 三个能力分组，同时审批判定覆盖所有八个工具。

## 八、审批 UI 与取消
- 现有审批卡片 + 取消信号机制保留（已修好的 select! 不动）。
- 审批卡片补充显示 risk（High 红色徽章）+ workspace cwd + 是否联网。
- `OnWrite`/`OnRisk` 免审批时仍落审计 + 前端 toast 提示（可选）。

## 九、新增依赖
- Rust：`landlock`、`nix`（rlimit）、`async-trait`、`regex`（grep）、`globset`、`walkdir`。
- 不强依赖 `bwrap`（运行时探测 `which bwrap`）。

## 十、分阶段实施

1. [x] **Phase A 工具模块拆分 + trait + workspace cwd 打通**：重构 executor 为 trait 派发，executor 取 session→workspace cwd，无沙箱行为变化。抽象 `trait SandboxGuard`（先放空实现/路径白名单实现）。
2. [x] **Phase B 统一 risk/approval tier + 迁移**：重构 policy，risk 真正驱动审批，UI 显示 risk。
3. [x] **Phase C 新增 file_edit / list_files / grep / apply_patch**：只读搜索 + 精确编辑 + 统一补丁。
4. [x] **Phase D Landlock 沙箱 + 资源限额**：FS 隔离（Linux），workspace 外只读；Windows 降级路径白名单。
5. [x] **Phase E git 加固 + 网络策略 + bwrap 可选增强**：git env_clear/子命令限制，网络默认开但可关，bwrap 探测增强（`--unshare-net` 等）。
6. [ ] **Phase F（后续）Windows Job Object + Restricted Token**：Windows 平台更强隔离。

## 十一、实施结果与安全边界（2026-07-16）

- Phase A–E 已完成；对应提交：`4b4a056`、`1e65bcf`、`a02fa80`、`e070c4d`、`dc6cce5`。
- Rust 内置工具共 8 个：`shell`、`file_read`、`file_write`、`file_edit`、`list_files`、`grep`、`apply_patch`、`git`；Python sidecar 已声明同一组 schema。
- Linux 已验证 Landlock workspace 写边界、symlink 逃逸拒绝、rlimit 文件大小上限，以及 bubblewrap loopback 网络隔离。
- 原生文件工具由路径能力层保护；Landlock 仅施加给 shell/git 子进程，避免污染桌面主进程。
- 非 Linux 或 Landlock 不可用时，文件工具仍受严格路径检查；shell/git 的内核级文件隔离会降级。bubblewrap 不可用且网络关闭时仅能启发式拒绝已知网络动作，不能视为完整网络沙箱。
- Windows Job Object / Restricted Token 仍属于 Phase F，不在本轮实现范围内。

## 已决策点
- **沙箱主防线**：Landlock（Linux）；Windows 用路径白名单降级，后续 Phase F 引入 Job Object + Restricted Token。
- **网络默认**：默认开（`allow=true`）；用户可在工作区策略关闭，关闭后用 bwrap `--unshare-net` 或启发式拦截。
- **新工具范围**：file_edit + list_files + grep + apply_patch（codex 风格统一补丁）。

## 十二、会话级权限模式（2026-07-16）

聊天输入区在模型选择器旁新增会话级权限选择器，配置持久化到
`sessions.permission_mode`，默认值为 `auto`。角色卡 `ToolPolicy` 继续决定工具是否启用；
会话权限模式决定本次会话的审批自动化程度，以及 `full_access` 下的能力边界。
原有 `ApprovalTier` 暂保留为角色审批偏好与后续 Auto 决策模型的输入；当前人工审批入口以会话权限模式为准。

| 模式 | 当前行为 |
|---|---|
| `ask_for_approval` | 每次工具调用都请求用户批准 |
| `auto` | 决策模型尚未接入，本轮所有调用直接请求用户决定；High 风险调用标记为用户二次确认 |
| `accept_edits` | 文件读取、搜索、写入、精确编辑和补丁自动执行；Shell、Git 与未知工具仍请求批准 |
| `full_access` | 已启用工具不再请求人工审批；放开文件路径与网络隔离，但保留超时、输出截断、rlimit 与审计 |

审批事件额外携带 `permission_mode`、`approval_reason`、
`is_secondary_confirmation`，前端审批卡显示触发原因，并对 Auto 下的 High 风险操作使用明确的高危确认文案。
审计快照同时记录会话权限模式和当次有效 `ToolPolicy`。

### Auto 后续接入点

下一轮的“用户指定决策模型”子系统接入后，只替换 `auto` 的普通调用判定：

1. Low/Medium 调用提交给用户选定的决策模型，由该模型返回允许或拒绝。
2. 决策模型不可用、超时或输出无效时，失败关闭并请求用户决定。
3. High 风险调用即使被决策模型允许，仍必须向用户发送二次确认卡片。
4. 其余三种模式的语义保持不变。
