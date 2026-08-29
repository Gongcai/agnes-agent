# Android 本地 Agent 与 Linux Runtime 设计

> 状态：A1 开发中；x86_64 Ubuntu 24.04.4 完整 Runtime 已在模拟器完成内置安装、Agent Protocol 握手和模型流式回复验证，PRoot 源码构建已自动化；ARM64 实机制品与 Ubuntu rootfs 全自动构建流水线待完成
> 更新时间：2026-08-29
> 关联文档：`PROJECT.md`、`architecture.md`、`CLOUDFLARE_SYNC.md`、`TOOLS_AND_SANDBOX.md`、`OPERIT_ANDROID_RUNTIME_REFERENCE.md`

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

PRoot 是用户态路径转换和系统调用拦截机制，不是强安全边界，也不能提供真正的 root、namespace、cgroup 或 systemd。Android 的真实安全边界必须由 Android 权限、SAF 授权、Rust capability、工具审批、路径检查和资源限制共同提供。

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
│  PTY / process lifecycle            │
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

- 负责 SQLite、同步、E2EE、工具策略、审批、审计和会话状态；
- 负责把工具请求转换为 Android Linux Runtime 中的受控执行；
- 负责 Agent Protocol 的连接、取消、重连和状态恢复；
- 不把 API Key、同步凭证或 E2EE keyset 传入 Ubuntu 文件系统。

Python Agent 仍然只负责 prompt、模型推理、上下文和工具选择。它不直接访问 Android SQLite、SAF 或宿主文件；工具执行必须回到 Rust Core，再由 Rust 通过 Runtime 执行器完成。

Android Runtime Service 负责跨 Activity 生命周期维护运行时：

- 启动、停止、重启和健康检查 PRoot 会话；
- 管理 Agent 进程、PTY 和长任务；
- 应用退到后台时保持已授权任务运行；
- 进程被系统回收后记录状态，并在下一次启动时恢复或明确标记未完成任务；
- 通过 Binder/AIDL 风格的原生 IPC 暴露最小的生命周期和事件接口。具体实现可使用 Tauri Android plugin；不得把任意 Shell 接口暴露给其他应用。

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

Shell 和 Git 工具在 Android 上也通过同一个 PRoot rootfs 执行。Rust 先按现有工具策略检查
宿主工作区、cwd、写入边界、审批、超时和输出上限，再将当前工作区只读/读写边界映射为
Linux 的 `/workspace`，并把宿主 cwd 转换为 `/workspace` 下的相对路径。工具进程使用 APK
内置的 `libagnes_proot.so` 和静态 `libagnes_proot_loader.so`，不把应用数据库、其他会话目录
或未授权 Android 路径挂载进 Linux。文件、记忆和数据库工具仍由 Rust 直接执行，避免 Linux
进程绕过 Rust 的权限审计。

开发调试可以使用预置 rootfs 或本机生成的测试 rootfs。正式 APK/AAB 默认内置对应 ABI 的 Runtime；应用内更新或远端制品下载作为后续升级通道，不能成为首次使用的必要步骤。

Runtime ZIP 可使用 `pnpm build:android-runtime -- --rootfs <Ubuntu-rootfs> --abi <arm64-v8a|x86_64>`
生成。脚本会复制 rootfs 到临时目录，注入 `agent/app` 和记忆提示词，保留安全软链接并按
安装器相同的规则生成 SHA-256，并输出带 `manifest.json` 的 ZIP。依赖包、Python 和
系统库必须预先安装到输入 rootfs；脚本不会联网安装依赖，也不会读取宿主密钥。

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

- Python 3.12、`pip`/`uv` 所需的最小运行支持；
- Shell、基础文件工具、Git、SSH；
- Python Agent 依赖及现有 Agent WebSocket 协议；
- 后续按需加入 Node.js、编译工具、MCP 进程和更多 Skills 依赖。

不把桌面端的 Python sidecar 打包方式直接照搬到 Android。Android 需要维护一个可升级的 Linux Python 环境，但 Agent Protocol 和 Python 业务代码尽量保持同源。

## 5. Android 与 Linux 的文件互通

Android 不把整个 `/sdcard` 或 Android 私有目录默认暴露给 Linux。工作区分为两类：

1. **应用私有工作区**：默认绑定到 Agnes 私有目录，稳定、无需额外授权，适合 Agent 自己创建的文件。
2. **用户授权工作区**：用户通过 Storage Access Framework 选择目录，Android 保存 persistable URI 权限，Runtime 将授权范围映射为 Linux 的 `/workspace`。

### 5.1 Android 内置项目工作区

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
挂载。两者来自同一宿主目录，Shell、Git、文件工具和 Agent 对项目的修改保持一致。项目目录不在
rootfs 版本目录内，因此 Runtime 升级、回滚或重新安装不会删除用户项目。

该入口不默认授权整个 Android 文件系统，也不替代后续 SAF 外部目录绑定；它是 Android 本地小型
工具开发的低门槛默认路径。

同步的 `workspace` 只保存逻辑 ID、名称和仓库身份等字段；本机绑定保存绑定类型、URI 或本地路径、权限状态和校验时间。任何 Android URI、Linux rootfs 路径和宿主绝对路径都不得进入同步 payload、Agent prompt 或云端日志。

工具执行规则：

- 没有工作区绑定时，Android 可以查看工作区会话，但文件、Shell、Git 工具必须明确拒绝；
- SAF 绑定失效时，不把空路径或旧路径当作有效 cwd；
- Rust 在每次文件工具调用前重新验证绑定和 capability；
- PRoot 的路径映射只实现可用性，不替代 Rust 的路径授权；
- Android 系统能力通过窄化 Bridge 提供，例如文件选择、分享、剪贴板、通知和相机，不向 Python 暴露通用 Android API。

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
- [x] 完成现有 Agent Protocol 握手和模型流式回复；取消流程待单独验收。

### A2：Android 本地完整 Agent

- [x] Android Shell/Git 进程通过 PRoot Ubuntu rootfs 执行，并将会话工作区映射为 `/workspace`；
- [x] Android Code 模式在持久 Linux 用户目录创建项目，并挂载为 `/root/workspace`；
- Shell、文件读写、Git、Python、SSH 和记忆工具的完整功能与权限验收；
- 工具审批、sandbox、审计、超时、并发和输出上限；
- 应用私有工作区和 SAF 工作区；
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
- Shell、Git、Python 和 SSH 只能访问有效工作区；
- Android 未授权路径不可访问；
- Rust 工具审批不能被 Linux Shell 绕过；
- rootfs 下载中断、校验失败和升级失败不会替换当前可用版本；
- 超时、输出大小、并发和存储配额均有硬限制；
- UI 与 Runtime 断开后可以重连并恢复运行状态。

双设备同步、R2 制品和最终跨设备验收在 Android 主体完成后统一执行。当前远端只有测试数据，开发阶段允许按测试策略清理和重建，但验收仍必须覆盖完整协议行为。

## 10. 非目标

- Android 内置完整 systemd、真实 root 或通用容器运行时；
- 默认开放整个 Android 文件系统；
- 让 Python 绕过 Rust 直接操作宿主数据库或文件；
- 把 SSH 作为唯一的 Android Agent 运行方案；
- 在 A0/A1 同时完成完整本地模型、Docling PDF 解析和所有 Android 自动化能力。
