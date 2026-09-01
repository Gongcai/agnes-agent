# Android 本地 Agent 与 Linux Runtime 设计

> 状态：A1 开发中；x86_64 Ubuntu 24.04.4 完整 Runtime 已在模拟器完成内置安装、Agent Protocol 握手和模型流式回复验证，PRoot 源码构建已自动化；ARM64 实机制品、用户可见 Linux PTY 与 Ubuntu rootfs 全自动构建流水线待完成
> 更新时间：2026-09-01
> 关联文档：`PROJECT.md`、`architecture.md`、`CLOUDFLARE_SYNC.md`、`TOOLS_AND_SANDBOX.md`、`OPERIT_ANDROID_RUNTIME_REFERENCE.md`

> 产品决策（2026-09-01）：Android 继续采用 Agnes 的 Agent-first 模型，但用户不是 Agent 的附属审批者。
> 用户必须能通过独立 PTY 完整使用内置 Ubuntu，进行包管理、编译、调试、运行服务和文件管理；
> PTY 是用户访问 Ubuntu 系统及其文件的主要门户。正式版接受内置完整 Runtime 的安装包和磁盘体积。
> Agnes 提供接近 Operit 的宽泛 Linux 基础能力和可选挂载，不替用户或 Agent 预先决定工具链、依赖
> 安装方式或 workspace 隔离策略；硬限制只来自 Android 本身和用户明确授予的资源范围。

## 1. 目标

Android 端最终必须支持与桌面端同等级别的本地 Agent 工作流：Agent、工具、记忆、工作区和任务执行可以在手机本地完成。第一版可以先交付聊天、同步和基础运行时接口，但不能把 Android 永久定义为只能查看历史的轻客户端。

Android 与桌面端共享 Agent Protocol、数据模型、工具策略和同步 E2EE；两端的执行环境不同：

- 桌面端在宿主操作系统上运行 Rust Core、Python Agent 和工具；
- Android 端由 Rust Core 协调一个独立的 Linux Runtime，在其中运行 Python Agent 和 Linux 工具；
- SSH 仍作为连接桌面 Agent 的兼容能力，但不再是 Android 获得完整 Agent 能力的主要路径；
- Cloudflare 只负责同步和加密大对象，不运行 Python Agent、LangGraph、Shell、Git 或 MCP。

## 2. 参考方案与选型

Operit 的公开实现提供了有价值的架构参考：Android 主应用与独立 terminal core 模块分离，通过 Service/AIDL 管理终端会话和后台任务，并在 Android 上运行 Ubuntu 24.04 ARM64 用户空间；其当前公开 Service 默认仍与主应用同进程。主仓库和终端核心的许可证、依赖打包方式和功能范围不适合直接复制到 Agnes；Agnes 只参考运行时隔离、生命周期管理和 IPC 边界。公开实现、资产布局、初始化流程、差异和复用边界详见 `OPERIT_ANDROID_RUNTIME_REFERENCE.md`。

候选方案比较：

| 方案 | 结论 | 原因 |
|---|---|---|
| PRoot + Ubuntu ARM64 用户空间 | 首选 | 不要求 root，能复用现有 Python Agent、Shell、Git、SSH 和 Node 生态 |
| 完整 Fork Termux | 不作为首选 | 能力成熟，但包名、插件、构建、分发和许可证负担明显增加 |
| Chaquopy / 嵌入 Python | 不作为 Linux Runtime | 可以嵌入 Python，但不是完整 Linux，原生依赖、包管理和工具兼容性不足 |
| Android 容器 / Podman | 不作为基础方案 | Android 设备通常不能依赖 rootless namespace、cgroup 和完整容器能力 |
| QEMU 虚拟机 | 保留研究 | 隔离较强，但体积、启动速度、性能和功耗不适合当前版本 |
| 自研完整 Linux 运行时 | 长期研究 | 控制力强，但会重新承担终端、包管理、rootfs 和工具生态的维护成本 |

PRoot 是用户态路径转换和系统调用拦截机制，不是强安全边界，也不能提供真正的 root、namespace、cgroup 或 systemd。Android 的真实安全边界来自 Android UID/权限、SELinux、scoped storage/SAF 和 Keystore；Rust capability、审计、用户可调授权与资源限制用于表达和管理这些边界，不能把完整 Linux 人为缩成命令白名单。

## 3. 目标架构

```text
┌────────────────────────────────────┐
│ Android Tauri App                   │
│  React UI                           │
│  Rust Core                          │
│  SQLite + E2EE + Sync               │
│  Tool policy / approval / audit     │
└────────────────┬───────────────────┘
                 │ Tauri Android bridge
                 ▼
┌────────────────────────────────────┐
│ Android Runtime Service             │
│  Foreground Service                 │
│  Runtime supervisor                 │
│  User PTY + hidden exec lifecycle  │
│  SAF / Keystore / notification      │
└────────────────┬───────────────────┘
                 │ authenticated local IPC
                 ▼
┌────────────────────────────────────┐
│ PRoot + Ubuntu ARM64 userspace     │
│  Python 3.12 + Agnes Agent         │
│  shell / git / ssh / node           │
│  Android workspace mount            │
└────────────────┬───────────────────┘
                 │ Agent Protocol
                 ▼
          LangGraph + LiteLLM
```

### 3.1 运行时职责

Rust Core 继续是应用资源和数据的唯一所有者：

- 负责 SQLite、同步、E2EE、用户可调工具授权、审计和会话状态；
- 负责把工具请求转换为 Android Linux Runtime 执行，并保持取消、超时和结构化结果；
- 负责 Agent Protocol 的连接、取消、重连和状态恢复；
- 不把 API Key、同步凭证或 E2EE keyset 传入 Ubuntu 文件系统。

Python Agent 仍然只负责 prompt、模型推理、上下文和工具选择。它不直接访问 Android SQLite、SAF 或宿主文件；工具执行必须回到 Rust Core，再由 Rust 通过 Runtime 执行器完成。

Android Runtime Service 负责跨 Activity 生命周期维护运行时：

- 启动、停止、重启和健康检查 PRoot 会话；
- 管理用户可见 PTY、Agent hidden exec 和长任务；两者使用独立的 session namespace、stdin、输出流和取消信号；
- 应用退到后台时保持已授权任务运行；
- 进程被系统回收后记录状态，并在下一次启动时恢复或明确标记未完成任务；
- 通过 Binder/AIDL 风格的原生 IPC 暴露最小的生命周期和事件接口。具体实现可使用 Tauri Android plugin；不得把任意 Shell 接口暴露给其他应用。

### 3.2 用户可见 Linux 终端

Android Local 不只提供 Agent 后台工具调用，还提供一个用户可以随时打开的内置 Ubuntu 终端。
这个终端是用户访问内置 Ubuntu 系统、开发工具和文件的一等门户，不需要先触发 Agent handoff；
用户可以直接使用 shell、`apt`、`dpkg`、Python、Git、SSH、编译器和本地服务。Agnes 保持
Agent-first 的产品入口不等于限制人工开发能力：Agent 与用户可以操作同一项目和 Ubuntu 实例，
两者使用不同的会话传输和生命周期，但默认共享同一套 Linux 工具、文件 namespace 和用户授权。

终端会话由 Runtime Service 持有，不依附某个 Activity。每个会话至少绑定以下状态：

- 不透明的 `terminal_session_id`、逻辑 workspace ID 和 Runtime 版本；
- guest `HOME`、当前 cwd、PTY 窗口行列数和前台/后台状态；
- PTY 输出的单调递增序号、最近状态和退出原因，以便 UI 重连后补齐未读输出；
- 资源配额、网络策略和 fake-root 标记，供 UI 展示并写入审计事件。

用户终端 API 的最小操作是 `create`、`list`、`subscribe_output`、`write`、`resize`、`signal`、
`reconnect` 和 `close`。PTY 使用原始字节流传输输入和输出；Ctrl+C 发送到该会话的前台进程组，
resize 更新同一 PTY 的窗口大小，不能通过模拟另一条 shell 命令实现。重连只能订阅已有会话，不能
重放用户输入；Runtime 重启或进程被系统回收后，旧会话标记为 interrupted，UI 必须明确显示并允许
创建新会话。

用户终端与 Agent hidden exec 的边界如下：

| 执行平面 | 入口 | stdin/输出 | 生命周期 | 权限语义 |
|---|---|---|---|---|
| User terminal | 用户终端 UI | 独占 PTY 字节流 | 由用户打开、重连、关闭 | 与 Agent 共享用户选择的 Runtime 能力，仍受 Android/SAF 边界 |
| Agent hidden exec | Agent `shell`/工具调用 | 默认无 stdin，结构化输出 | timeout、取消和进程组清理 | 使用同一基础 Linux 能力；保留审计、取消和资源上限，不另设人为工具禁区 |

用户主动输入的终端命令不重复走 Agent 工具审批，但不能绕过 Android 应用沙箱、用户选择的 Runtime
挂载边界、资源配额和网络边界。用户对自己的 Ubuntu 实例拥有完整开发控制，可以直接安装或删除系统包、
创建语言环境、编译原生程序、修改 guest 配置并启动监听服务；这些操作不需要按 Agent 工具调用
逐条审批，也不要求 Agnes 代替用户选择全局或 workspace 环境，但仍记录 Runtime 审计事件，并明确提示磁盘、网络暴露和 Runtime 重置影响。包管理状态
属于当前用户 Runtime 的可变层，升级前记录包清单和关键配置；恢复内置 Runtime 时必须让用户选择
保留、导出或丢弃这些变更，不能静默覆盖。

Agent hidden exec 默认使用非交互协议，但拥有与用户终端一致的 Ubuntu 基础命令、包管理、网络和
文件能力；它不应因为是 Agent 就被强制切换到另一套工具或依赖模型。需要输入时必须返回
`requires_interactive_terminal`，经用户明确批准后才创建或绑定 handoff 会话，不能偷偷接管用户已有终端。

## 4. Linux 用户空间

### 4.1 Rootfs

首个正式发布目标平台为 `arm64-v8a`，开发模拟器另维护 `x86_64`。Ubuntu 24.04 作为默认用户空间，面向普通用户的正式构建按目标 ABI 内置一份可验证 Runtime 安装包，首次使用时自动安装；开发构建可以显式关闭内置 Runtime 并保留手动安装通道：

- 安装包带版本、架构、压缩格式和 SHA-256 manifest；
- 下载或内置资源安装到应用私有目录；
- 解压限制文件数量、总大小和路径穿越；
- 安装使用临时目录，校验通过后原子切换版本目录；
- 启动失败自动回退到上一份可用版本；
- rootfs 版本与 Python Agent 运行时版本明确绑定；
- 不允许从未校验的远端地址直接执行安装脚本。

当前开发版安装包契约为 ZIP：根目录必须包含 `manifest.json`，Linux 用户空间文件放在
`rootfs/` 下。manifest 使用 schema version 1，字段如下：

```json
{
  "schemaVersion": 1,
  "runtimeVersion": "ubuntu-24.04.1",
  "platform": "android",
  "abi": "arm64-v8a",
  "rootfsSha256": "<按相对路径排序后的 rootfs 文件摘要>",
  "rootfsSize": 123456789,
  "agentVersion": "0.1.0"
}
```

摘要按 `相对路径 + NUL + 文件内容` 的顺序计算，软链接的“文件内容”为链接目标文本，
路径按字典序排列；目录不参与摘要。
安装器会拒绝绝对路径、`..` 路径、越出 rootfs 的软链接、超过 200,000 个文件、单文件超过
512 MiB 或 rootfs 总文件内容超过 2 GiB 的制品。文件先写入应用私有目录下的 staging
版本，摘要和大小通过后再原子切换 `current` 指针，旧版本保留在 `versions/`，上一版本
记录在 `previous`。

PRoot runner 当前约定以下文件由 Android 包和 rootfs 制品共同提供：

- `libagnes_proot.so`：由目标 ABI 的 `jniLibs` 打入 APK，安装后从 Android `nativeLibraryDir` 执行；不从应用可写 rootfs 执行 ELF；
- `libagnes_proot_loader.so`：同 ABI 的静态 PRoot ELF loader；通过 `PROOT_LOADER` 直接使用，避免 PRoot 在 Android 应用私有目录临时提取 loader；
- `usr/bin/python3`：Linux 用户空间内的 Python 解释器；
- `usr/bin/pip3` 和 `usr/bin/pip`：必须解析到真实的 pip 脚本，不能是自指软链接；打包器会在 staging rootfs 中规范化为 `pip3.12`/`pip3`；Runtime 保留 Ubuntu 的 PEP 668 默认语义，但 Agnes 不额外拦截用户或 Agent 明确选择的安装方案；
- `opt/agnes-agent/app/main.py`：与桌面端同源的 Python Agent；
- `usr/bin/bash`、`usr/bin/git`、Python 和 SSH 等 Agent 工具依赖也必须位于 rootfs；
- Agent 依赖、证书和启动配置也必须位于 rootfs 内，不写入 Android 私有目录中的明文配置。

Rust 的公网 HTTPS 请求使用 Rustls，同时加载系统根证书和内置 Mozilla WebPKI 根证书。
Android 应用沙箱通常没有桌面 Linux 的 OpenSSL CA 文件路径，只依赖 native roots 会导致
所有模型服务商的 TLS 握手失败；WebPKI roots 作为跨 Android 版本的基础信任集合，系统根
证书仍保留用于企业网络或用户安装的 CA。连接错误会保留 reqwest 的底层错误链，便于区分
DNS、TLS、连接拒绝和超时。

Rust 会把应用私有工作区绑定为 Linux 的 `/workspace`，通过一次性 token 和本地回环
WS 连接 Agent Protocol。Android Runtime 不向其他应用暴露 Service，停止 Runtime 时会
先回收 Agent 子进程，再移除前台通知。

Shell 和 Git 工具在 Android 上也通过同一个 PRoot rootfs 执行。当前实现仍先按现有工具策略检查
宿主工作区、cwd、写入边界、审批、超时和输出上限，再将当前工作区只读/读写边界映射为
Linux 的 `/workspace`，并把持久项目根目录额外挂载为 `/root/workspace`，把宿主 cwd 转换为
`/workspace` 下的相对路径。工具进程使用 APK
内置的 `libagnes_proot.so` 和静态 `libagnes_proot_loader.so`，不把应用数据库、其他会话目录
或未授权 Android 路径挂载进 Linux。文件、记忆和数据库工具仍由 Rust 直接执行，避免 Linux
进程绕过 Rust 的权限审计。这是待替换的基线：目标实现保留业务数据库和秘密边界，但移除只允许
workspace 的默认限制，让 Rust 文件工具与 Linux shell 共用完整 Runtime mount table。

运行时使用 PRoot `-0` fake-root，让 `apt`/`dpkg` 在隔离 rootfs 内获得一致的 root 语义；Android
内核仍以应用 UID 执行进程，因此这不是 Android root，也不能越过应用沙箱。`/dev`、`/proc` 和
应用私有的 `android-runtime/tmp/shm` 分别挂载到 Linux 环境，其中最后一项提供 POSIX semaphore
所需的 `/dev/shm`；Android 无法在进程内创建真实 tmpfs，因此启动前必须准备该目录。工具和 Agent
固定注入 `LANG=C.UTF-8`、`LC_ALL=C.UTF-8`、`TMPDIR=/tmp` 与 `UV_LINK_MODE=copy`。共享存储
工作区仍可读写，但其 FUSE 限制意味着部分设备上不能依赖软链接或直接执行其中的脚本；Python
虚拟环境和需要直接执行的工具可以选择应用私有 execution store，最终由能力探测和用户/Agent 决定。

每次启动 Agent 前，Rust 会使用与正式 Agent 相同的 PRoot、loader、`/dev`、`/proc` 和
`/dev/shm` 挂载，运行一次最小 Python `multiprocessing.SimpleQueue` 探测。先以不带
`--link2symlink` 的最小模式启动；只有最小模式失败而兼容模式成功时才为本次 Runtime 启用该参数。
Agent 与工具进程共享同一份不可变 PRoot 配置，不能分别拼接能力参数。探测同时读取关键 `/proc`
节点，并在 rootfs、environment store、Home workspace 和应用私有项目 workspace 实测读写/rename、
symlink、hardlink 与 direct exec。结果写入不含宿主路径的 `android-runtime/capabilities.json`。
探测失败时不启动前台 Runtime Service 或 Agent，并把 PRoot、Python、共享内存和 rootfs 兼容性
错误返回给 UI。

Runtime 启动会创建持久的 `android-runtime/user/environments` 与 `user/cache/uv`，作为后续 uv
environment service 的 app-private execution store。当前阶段只建立目录与能力档案，尚未自动绑定
项目环境。全局 Python 保留 Ubuntu PEP 668；历史版本若存在 Agnes 精确生成的
`[global] break-system-packages = true` 配置会在启动时迁移删除，用户自定义 pip 配置不会被覆盖。

Agent 和 Android Linux 工具进程都通过 `env_clear` 启动，仅保留协议、PRoot 和运行时所需的
显式变量；工具策略允许的环境变量也必须经过白名单过滤。这样 Android 宿主的路径、代理和
其他配置不会意外进入 Ubuntu 用户空间，同时固定注入 `HOME`、`TERM`、`PATH`、locale、
`TMPDIR` 和 `NO_COLOR`。

开发调试可以使用预置 rootfs 或本机生成的测试 rootfs。正式 APK/AAB 默认内置对应 ABI 的 Runtime；应用内更新或远端制品下载作为后续升级通道，不能成为首次使用的必要步骤。

Runtime ZIP 可使用 `pnpm build:android-runtime -- --rootfs <Ubuntu-rootfs> --abi <arm64-v8a|x86_64>`
生成。脚本会复制 rootfs 到临时目录，注入 `agent/app` 和记忆提示词，保留安全软链接并按
安装器相同的规则生成 SHA-256，并输出带 `manifest.json` 的 ZIP。打包时还会补齐
`rootfs/dev/shm` 和 `rootfs/tmp`，并将 Python 的 `pip3`/`pip` 入口规范化为可解析的真实
脚本，拒绝自指软链接。依赖包、Python 和系统库必须预先安装到输入 rootfs；脚本不会联网
安装依赖，也不会读取宿主密钥。

宿主 PRoot 可使用 `pnpm build:android-proot -- --abi <arm64-v8a|x86_64>` 从锁定的 Termux
PRoot 与 Samba talloc commit 构建。版本、commit、Ubuntu Base URL 和 SHA-256 记录在
`android-runtime/sources.lock.json`；构建会检查 ELF ABI、PIE、Android linker 和动态依赖，
同时检查静态 loader 的 ABI、ELF 类型和入口点，并生成许可证副本及 SPDX SBOM。当前 Ubuntu rootfs 的 apt 与 Python 锁定依赖安装仍需人工
执行，后续需要补齐无特权、可复现的 rootfs 构建脚本。

Android 构建从仓库根目录下未纳入 Git 的 `android-runtime/<abi>/` 读取三个制品：
`runtime.zip`、`libagnes_proot.so` 和 `libagnes_proot_loader.so`。`scripts/prepare-android-runtime-assets.mjs` 在构建前只把
目标 ABI 暂存到生成的 Android 工程，防止 universal APK 意外带入两套大体积 rootfs。
`pnpm android:build -- --target <aarch64|x86_64>` 默认要求制品完整；仅开发无 Runtime 的
界面调试可显式设置 `AGNES_ANDROID_RUNTIME_OPTIONAL=1`。直接运行 Tauri Android 命令时，
可用 `AGNES_ANDROID_TARGET=x86_64 AGNES_ANDROID_RUNTIME_OPTIONAL=1 pnpm tauri android dev`
明确目标并允许本地缺少制品；未显式允许时，构建 hook 会因制品缺失而终止构建。

应用通过 Android plugin 检查内置资产和宿主 PRoot。首次点击启动时，plugin 先把压缩包
安全复制到应用私有目录，Rust 再复用与外部 ZIP 相同的 manifest、ABI、摘要、大小和路径
校验进行安装；已安装版本不会因 APK 内置资产存在而被重复覆盖。外部 ZIP 安装和上一版本
回滚继续作为开发、升级与恢复通道。

### 4.2 工具与依赖

本地完整 Agent 的第一批 Linux 能力：

- Python 3.12、`pip`、`uv` 和 venv；
- Shell、完整基础文件工具、Git、SSH、证书和常用编译工具；
- Python Agent 依赖及现有 Agent WebSocket 协议；
- Runtime 设置提供 Node.js/npm/fnm、Rust/rustup、MCP 进程和更多 Skills 依赖的可选预装入口；它们
  安装到同一个用户可配置 Ubuntu，不拆成 Agent 专用工具套件。

不把桌面端的 Python sidecar 打包方式直接照搬到 Android。Android 需要维护一个可升级的 Linux Python 环境，但 Agent Protocol 和 Python 业务代码尽量保持同源。

## 5. Android 与 Linux 的文件互通

Android 不可能靠 PRoot 绕过应用 UID、SELinux、scoped storage 和用户授予的存储权限，但在这些
真实边界以内，Agnes 不再额外收窄成“只能访问当前 workspace”。目标是让用户和 Agent 在 guest
看到稳定、连贯、接近普通 Linux 的文件系统，并让 shell、文件工具、Git、编辑器和脚本共用它。

Runtime 使用按资源授权区分的 mount profile，执行主体不再决定可用工具范围：

| Profile | 默认可见内容 | 可选内容 | 用途 |
|---|---|---|---|
| Base Linux profile | rootfs、用户 home、workspace、专用 tmp 和必要伪文件系统 | 无需额外授权 | 用户和 Agent 的共同基础环境 |
| Extended storage profile | Base profile | 用户明确授权的 SAF 目录或 emulated storage | 用户和 Agent 均可使用；由用户选择是否开启 |

挂载授权以稳定 resource ID 记录，并同时对 User terminal 与 Agent 生效；不再人为制造“用户可见但
Agent 永远不可见”的文件模型。用户可以单独关闭 Agent 对某个 resource 的访问，但默认不把 Agnes
的策略限制误写成 Linux 能力限制。

目标 mount table 向 Operit 普通 PRoot 模式靠近：

| Android source | Guest target | 说明 |
|---|---|---|
| Runtime rootfs | `/` | 用户和 Agent 可修改的完整 fake-root Ubuntu |
| Agnes runtime data subtree | 对应 `/data/user/<user>/<package>/runtime` 与 `/data/data/<package>/runtime` | 只映射 Runtime 自身数据，不暴露 SQLite、API Key、同步凭证或 E2EE 数据 |
| Runtime home | `/root` 及其稳定宿主映射 | 工具链、配置、缓存和用户文件可持久化 |
| host `/dev`、`/proc`、`/sys`、`/dev/pts` | 同路径 | 按 source/capability probe 挂载，缺失节点使用明确标记的 fallback |
| host `/proc/self/fd*` | `/dev/fd`、`/dev/stdin`、`/dev/stdout`、`/dev/stderr` | 保持常见 CLI 的文件描述符语义 |
| `/storage/emulated/<user>` | `/sdcard` 与同一原始路径 | 用户授予 All files access 后启用，两个别名指向同一 resource |
| host `/data/local/tmp` | `/data/local/tmp` | 与 Operit 一样先探测；仅在当前 app UID 实际可访问且用户启用兼容挂载时加入 |

普通 PRoot 不映射整个 `/data/data` 或 `/data/user`，也不尝试访问其他应用私有数据。若未来提供
root/chroot 实验模式，必须作为完全不同的高风险产品能力重新决策，不能由普通 Runtime 自动升级。

工作区分为两类：

1. **应用私有工作区**：默认绑定到 Agnes 私有目录，稳定、无需额外授权，适合 Agent 自己创建的文件。
2. **用户授权工作区**：用户通过 Storage Access Framework 选择目录，Android 保存 persistable URI 权限，Runtime 将授权范围映射为 Linux 的 `/workspace`。

SAF 返回的是 `content://` 文档树，不是可直接交给 PRoot `-b` 的 POSIX 路径。对于本机 emulated
storage，优先使用用户授予的 All files access 和真实路径直接 bind，避免把安装依赖、编辑、目录
浏览和脚本运行拆成多套文件模型。只有云盘或其他确实不提供 POSIX 路径的 DocumentsProvider 才使用
app-private mirror/import-export；镜像后端需要冲突检测、原子写回、删除确认和授权失效处理，不能把
URI 字符串伪装成普通路径。

### 5.1 用户共享存储宽挂载

为了支持用户和 Agent 直接处理 Downloads、媒体和已有源码，Runtime 初始化向用户提供“共享存储访问”
选项：在 Android 系统设置中由用户授予相应权限后，将当前 Android user 的 emulated storage
同时挂到 `/sdcard` 和 `/storage/emulated/<user>`，与 Operit 和 Android 常见路径保持一致；可额外
提供 `/mnt/android` 友好别名，但内部策略只识别同一个 resource ID。是否启用、开放给哪些会话由
用户选择，不由 Agnes 预设工作流。

该模式必须满足：

- Android 授权前不可用，Runtime 首次设置时主动提供并解释其读取、修改和删除范围；随时可以撤销；
- 不挂载整个 `/data/data`、`/data/user` 或其他应用私有目录；Agnes runtime data 使用精确子目录映射，
  `/data/local/tmp` 仅在 probe 成功且用户启用兼容挂载时加入；
- 默认可以同时供用户终端、Agent hidden exec 和 MCP 使用；用户可按 resource ID 撤销某类会话，
  但不提供与 Linux 基础能力无关的强制 workspace 隔离；
- 启动时探测 read/write/rename、symlink、direct exec 和 case sensitivity，按实际能力报告；共享存储
  通常不适合 venv、原生构建输出或密钥，app-private execution store 作为推荐位置但不强制；
- 终端启动的脚本、依赖安装钩子和本地服务与用户本人拥有相同的挂载可见性，因此 UI 必须把当前
  广域挂载持续显示在终端状态中，关闭权限后立即阻止新会话继承；
- Google Play 发行不得默认依赖 `MANAGE_EXTERNAL_STORAGE`。若应用商店政策不允许 Agnes 申请
  All files access，则 Play 版本只提供 SAF/app-private 模式，完整共享存储模式留给符合政策的渠道。

### 5.2 Android 内置项目工作区

Android Code 模式提供一个面向小工具开发的内置项目入口。Runtime 启动时会在应用私有数据目录中
预先创建持久目录：

```text
<app-data>/android-runtime/user/workspace/
```

用户点击“添加项目”时输入项目名称，应用只接受 1-64 个 ASCII 字符，首字符必须是英文字母或
数字，其余字符只能是英文字母、数字、`_` 和 `-`。名称会直接作为目录名使用；中文、空格、点号、
路径分隔符、以连字符开头的名称和同名项目都会被拒绝。

创建的项目目录同时以两种路径提供给 Runtime：

```text
宿主持久目录：<app-data>/android-runtime/user/workspace/<project-name>
Linux 用户目录：/root/workspace/<project-name>
当前会话工作区：/workspace
```

其中 `/root/workspace` 是 PRoot 对持久项目根目录的挂载，`/workspace` 是当前 Code 会话的工作区
挂载；Agent 主进程和 Rust 发起的 Shell/Git/Python 工具进程都使用这两个挂载。两者来自同一宿主目录，
Shell、Git、文件工具和 Agent 对项目的修改保持一致。项目目录不在
rootfs 版本目录内，因此 Runtime 升级、回滚或重新安装不会删除用户项目。

该入口不默认授权整个 Android 文件系统，也不替代后续 SAF 外部目录绑定；它是 Android 本地小型
工具开发的低门槛默认路径。

同步的 `workspace` 只保存逻辑 ID、名称和仓库身份等字段；本机绑定保存绑定类型、URI 或本地路径、权限状态和校验时间。Android URI 和 app-private 宿主实现路径不得进入同步 payload、Agent prompt 或云端日志；`/root`、`/workspace`、`/sdcard` 和 `/storage/emulated/<user>` 等稳定 guest 路径可以正常提供给 Agent。

工具执行规则：

- 没有工作区绑定时，文件、Shell 和其他 Linux 工具仍可在 `/root`、rootfs 和已授权挂载中工作；
  Git 只有在当前目录确实是仓库时才执行仓库操作，不因缺少 Agnes workspace 记录而整体禁用；
- SAF 绑定失效时，不把空路径或旧路径当作有效 cwd；
- Rust 在每次文件工具调用前重新验证绑定和 capability；
- PRoot mount table 与 Rust 路径解析共享同一 canonical resource；Rust 只执行用户明确配置的资源撤销，
  不再添加仅针对 Agent 的隐式 workspace 限制；
- Android 系统能力通过窄化 Bridge 提供，例如文件选择、分享、剪贴板、通知和相机，不向 Python 暴露通用 Android API。

Android UI 需要提供面向统一 Runtime namespace 的文件入口：可以直接浏览 `/root`、项目、rootfs
和已授权共享存储，打开目录、编辑文件并把选中的 guest 路径交给终端或 Agent。实现可以采用
app-internal 文件浏览器并按需增加只读/读写 DocumentsProvider；无论采用哪种 UI，都必须复用同一
mount table 和 GuestPathResolver，不能再维护一套 SAF URI/缓存副本路径。

## 6. 模型运行模式

“本地 Agent”与“完全离线 Agent”分开规划：

- 第一阶段支持手机本地运行 Agent、工具和记忆，但模型可以通过 LiteLLM 调用用户配置的远程 Provider；
- 后续再评估 `llama.cpp`、GGUF、MNN 等 Android 本地推理；
- 本地模型要单独评估 APK/模型体积、内存、量化质量、发热、后台限制和多 ABI 分发；
- 云端模型 API Key 只存 Android Keystore，不写入 SQLite、rootfs 或同步 payload。

Android SecretStore 通过窄化 Tauri plugin 接入 `AndroidKeyStore`。Keystore 中只生成不可导出的
AES-256-GCM 主密钥；secret ID 经 SHA-256 后作为应用私有 `SharedPreferences` 索引，secret
值使用随机 nonce 加密，AAD 绑定原始 secret ID。密文偏好文件从云备份和设备迁移中排除，
避免密文被恢复到没有原主密钥的设备。Renderer 仍只能读取“是否已配置”和 backend 状态，
不能读取任何已保存 secret。

## 7. 与桌面端的互通

Android 有两种执行模式：

| 模式 | 执行位置 | 用途 |
|---|---|---|
| Android Local | 手机 Linux Runtime | 手机独立完成聊天、记忆和工具任务 |
| Desktop Remote | 桌面端 Agent Runtime | 手机连接桌面工作区，使用桌面性能和本地目录 |

两种模式都通过统一 Agent Protocol 表达流式回复、工具请求、审批、取消、错误和恢复。SSH/隧道能力属于 Desktop Remote adapter，不改变 Android Local 的基础架构。

## 8. 分阶段路线

### A0：Android 基础客户端与 Runtime 接口

- [x] Tauri Android scaffold、ARM64 构建链和 debug APK；
- [x] Android 专用 Tauri 配置，跳过桌面 sidecar 打包；
- [x] 移动端 drawer、聊天、Agent、Session、Memory 页面复用现有 UI；
- [x] Android library target、桌面/Android 条件编译和 Rustls 网络栈；
- [x] `AgentRuntime` 抽象，将桌面 sidecar 与 Android Runtime 解耦；
- [ ] Android SQLite、E2EE、设备配对和同步 bootstrap 的真机验收；
- [x] Android Keystore 接入，完成密文备份排除及模拟器读写/覆盖/隔离/删除测试；
- [ ] workspace binding 数据模型和 SAF 权限状态；
- 可先使用测试数据，不把双设备完整验收作为 A0 阻塞条件。

### A1：Linux Runtime 原型

- [x] Android Foreground Service 基础生命周期与通知（独立进程 supervisor 待补充）；
- [x] PRoot runner 启动路径与最小 Ubuntu rootfs 文件契约；
- [x] rootfs manifest、Hash 校验、路径/资源限制、原子安装；
- [x] 安装失败回滚和当前版本恢复；
- [x] 启动/停止 Python Agent 的 PRoot 接线；
- [x] 按 ABI 暂存 APK 内置 Runtime、首次自动安装和 `nativeLibraryDir` PRoot 执行路径；
- [x] 从锁定上游源码自动构建 ARM64/x86_64 PRoot，生成许可证清单与 SPDX SBOM；
- [x] 构建 x86_64 Ubuntu 24.04.4 完整 Runtime，并在模拟器验证首次安装与启动；
- [ ] 自动化 Ubuntu rootfs、apt 和 Python 锁定依赖的可复现构建；
- [ ] 构建并在 ARM64 设备验收完整 Runtime 制品，完成发布签署；
- [x] Rust Core 与 Agent 的本地认证 WS IPC 接线；
- [x] PRoot `-0` fake-root、共享启动配置和条件 `--link2symlink` capability probe；
- [x] rootfs/workspace/environment store 文件系统能力档案与旧全局 pip override 迁移；
- [x] 完成现有 Agent Protocol 握手和模型流式回复；取消流程待单独验收。
- [ ] Runtime Service 建立 user terminal 与 Agent hidden exec 的独立 session namespace 和事件模型；

### A2：Android 本地完整 Agent

- [x] Android Shell/Git 进程通过 PRoot Ubuntu rootfs 执行，并将会话工作区映射为 `/workspace`；
- [x] Android Code 模式在持久 Linux 用户目录创建项目，并挂载为 `/root/workspace`；
- [ ] 从 Android UI 随时创建、列出、重连和关闭用户可见 Linux PTY；
- [ ] 支持原始输入、输出订阅、当前 cwd、窗口 resize、Ctrl+C、退出状态和中断原因展示；
- [ ] 用户终端可直接运行 `bash`、`apt`、`dpkg`、`python3`、`git` 和 `ssh`，并遵守 Runtime 资源与网络边界；
- [ ] user terminal 与 Agent hidden exec 不共享 stdin、输出流、取消信号或 session ID；
- [ ] Shell、文件读写、Git、Python、SSH、MCP 和用户 PTY 共用完整 Runtime mount table；
- [ ] User terminal 与 Agent 默认可修改 guest rootfs、home 和 runtime data，并可直接安装依赖、
  编辑文件、打开目录和运行脚本；
- [ ] 用户授予 All files access 后，将 emulated storage 同时映射为 `/sdcard` 与 Android 原始路径，
  Agent、PTY、Rust 文件工具和 UI 文件入口对同一文件解析一致；
- [ ] 提供 Runtime 文件浏览入口，可浏览 `/root`、项目、rootfs 和已授权共享存储并打开文件夹；
- 用户可调 resource 授权、审计、超时、并发和输出上限；
- 应用私有工作区、SAF 工作区和可选宽共享存储；
- Android Bridge 的最小系统能力；
- 后台任务、崩溃恢复和运行状态恢复；
- Skills/MCP 按 Android 资源和权限边界逐项接入。

### A3：跨设备与移动制品能力

- 桌面 Remote adapter 和 SSH 控制能力；
- R2 加密制品 Range 下载、完整校验、原子安装和设备状态回写；
- EPUB、知识库 artifact 和本地索引重建；
- 双设备并发、删除冲突、墓碑防复活和真实 E2EE 日志验收；
- 系统原生通知、后台同步和离线恢复；
- 再评估本地模型和更完整的 Android 文件/媒体能力。

## 9. 验收标准

Runtime 原型必须先满足：

- 后台运行时 Agent 能继续执行已授权任务；
- 前台服务停止会取消任务并清理子进程；
- App 被杀后能恢复或明确标记未完成运行；
- Python Agent 在 Ubuntu 用户空间中通过现有协议握手；
- Shell、Git、Python、SSH 和文件工具共享同一 Runtime namespace，可访问 rootfs、home、workspace
  以及用户已授权的扩展存储；
- Android 未授权路径不可访问；
- 用户可以查看并调整 Agent 的 resource 授权；路径别名不能绕过被明确撤销的 resource；
- rootfs 下载中断、校验失败和升级失败不会替换当前可用版本；
- 超时、输出大小、并发和存储配额均有硬限制；
- UI 与 Runtime 断开后可以重连并恢复运行状态。
- 用户无需经过 Agent handoff 即可打开内置 Ubuntu PTY；终端可完成输入、输出、resize、Ctrl+C、
  apt/dpkg 交互和 shell 退出，并在重连后保持会话状态可追踪；
- user terminal 与 Agent hidden exec 的 session ID、stdin、输出和取消不会互相混淆；

双设备同步、R2 制品和最终跨设备验收在 Android 主体完成后统一执行。当前远端只有测试数据，开发阶段允许按测试策略清理和重建，但验收仍必须覆盖完整协议行为。

## 10. 非目标

- Android 内置完整 systemd、真实 root 或通用容器运行时；
- 默认开放整个 Android 文件系统；
- 让 Python 绕过 Rust 直接操作宿主数据库或文件；
- 把 SSH 作为唯一的 Android Agent 运行方案；
- 在 A0/A1 同时完成完整本地模型、Docling PDF 解析和所有 Android 自动化能力。
