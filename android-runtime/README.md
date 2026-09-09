# Android Runtime 制品

此目录是 Android 应用构建时读取的本地制品入口。二进制制品不会提交到普通 Git，必须从
Ubuntu、PRoot 等上游源码独立构建。

上游版本、commit、Ubuntu Base 下载地址和 SHA-256 统一锁定在 `sources.lock.json`。源码补丁
位于 `patches/`，两者需要随代码提交；生成的 ELF、ZIP、许可证副本和 SBOM 仍由 `.gitignore`
排除。

目录结构：

```text
android-runtime/
├── arm64-v8a/
│   ├── runtime.zip
│   ├── libagnes_proot.so
│   └── libagnes_proot_loader.so
└── x86_64/
    ├── runtime.zip
    ├── libagnes_proot.so
    └── libagnes_proot_loader.so
```

`runtime.zip` 包含 Ubuntu 用户空间和 Agnes Python Agent。Runtime 同时承载两个执行平面：用户直接
操作的可见 Linux PTY，以及供 Agent 工具使用的 PTY 会话（与桌面端同一代码路径）；两者必须使用
独立会话和事件流。`libagnes_proot.so` 是 Android
宿主侧的 PRoot 可执行文件；使用原生库命名后，Android 会把它安装到应用只读、可执行的
原生库目录。`libagnes_proot_loader.so` 是同 ABI 的静态 ELF loader，运行时通过
`PROOT_LOADER` 直接使用，避免 PRoot 从应用私有临时目录提取 loader。

Android 的 Shell 和 Git 工具会复用 Agent 启动时的 PRoot rootfs。stdio MCP server 也在同一
Ubuntu guest 内启动：命令按 guest PATH（或 rootfs 内绝对路径）解析；发布制品预装
Node.js 22 LTS（`node`/`npm`/`npx`，锁定版本与 SHA-256 见 `sources.lock.json`），缺失程序会
在连接时返回明确的预装提示；server 进程使用与 Agent 工具相同的 mount table、干净环境和
`/root` 起始目录，密钥环境变量经 SecretStore 在启动时注入。用户终端和 Agent 工具共享同一份
Runtime launch spec，但不会共享 stdin、输出流或取消信号。Rust 会先完成工作区、cwd、
写入边界和审批检查，再将工作区映射为 Linux 内的 `/workspace`；工具不会获得其他 Android
应用私有目录或未授权路径。Agent 主进程和 Rust 发起的 Shell/Git/Python 工具进程都会将持久项目
根目录挂载为 `/root/workspace`，文件和记忆工具仍由 Rust 执行，Linux 进程不能绕过 Rust 审计。

Runtime supervisor 使用 PRoot `-0` 提供仅限 guest rootfs 的 fake-root，并在每次启动前先以最小
参数运行 Python/`/dev/shm` 探测。`--link2symlink` 不是固定参数：只有最小模式失败、带该参数的
兼容模式成功时才启用。最终能力值通过同一份启动配置传给 Agent 与工具执行器。

启动还会在 `android-runtime/capabilities.json` 记录关键 `/proc` 节点可读性，以及 rootfs、
environment store、Home workspace 和应用私有项目 workspace 的读写/rename、symlink、hardlink、
direct-exec 能力。档案不包含 Android 宿主绝对路径。持久 Python 环境和 uv cache 预留在
`android-runtime/user/environments` 与 `android-runtime/user/cache/uv`；全局 Python 保留 PEP 668，
Runtime 不再通过 `break-system-packages` 默认放开全局 pip。

## Android 项目工作区

Runtime 启动时会在应用私有数据下创建持久项目根目录：

```text
<app-data>/android-runtime/user/workspace/
```

Android Code 模式的“添加项目”要求输入目录名。名称规则为 1-64 个 ASCII 字符，首字符必须是
英文字母或数字，其余只能使用英文字母、数字、`_` 和 `-`。因此中文、空格、点号、路径分隔符、
以连字符开头的名称和已存在的同名目录都会被拒绝。

例如项目 `tiny-tool` 会得到：

```text
应用持久目录：<app-data>/android-runtime/user/workspace/tiny-tool
Linux 用户目录：/root/workspace/tiny-tool
```

会话启动后，选中的项目还会被挂载到 `/workspace` 作为当前工作目录。`/root/workspace/tiny-tool`
和 `/workspace` 来自同一份宿主目录，所以 Agent 的 Shell/Git 操作与 Rust 文件工具看到的是同一
份文件。项目不存放在 rootfs 版本目录中，Runtime 更新、回滚和重新安装不会覆盖它。

## 用户可见 Linux 终端

Android Runtime Service 必须提供独立于 Activity 的用户终端会话。用户可以随时从 Agnes 打开内置
Ubuntu shell，直接运行 rootfs 中的 `bash`、`apt`、`dpkg`、`python3`、`git` 和 `ssh`；这不是仅在
Agent 失败时才出现的 handoff 界面。

终端会话至少记录 `terminal_session_id`、workspace、guest `HOME`、cwd、Runtime 版本、PTY 行列数、
输出序号、退出状态和中断原因。接口需要覆盖创建、列出、输出订阅、原始输入、resize、Ctrl+C、
重连和关闭。输出订阅必须可从序号继续，重连不得重放用户输入；Runtime 或进程被回收后，会话标记
为 `interrupted`，不得假装仍在运行。

Agent 终端使用独立的 session namespace，与桌面端共用同一条 PTY 代码路径：`shell` 在持久 PTY
会话中执行，yield 超时后返回 session id 转后台轮询，`write_stdin` 发送输入或补读输出，
`stop_terminal` 与 run 取消（`stop_run`）负责进程组清理。用户终端命令不重复走 Agent 工具审批，
但仍受 Android 沙箱、PRoot 挂载、网络策略、资源配额和 provisioning 边界约束，并写入可审计的
运行事件。Agent 会话不能接管用户已有终端，两类会话互不共享 stdin、输出流、取消信号或 session ID。

当前 Rust Core 和 Android UI 已接入独立用户 PTY：终端使用 `user-terminal-*` 会话 ID、独立 registry、
cursor 输出补读、输入、resize、Ctrl+C、退出状态和重连列表；Runtime 停止时会回收全部用户 PTY。
进程重启后的旧 PTY 不会伪装成可恢复会话，后续需要持久化 interrupted 元数据并完成独立 Service/真机验收。

Runtime rootfs 除 Agent 依赖外，发布制品还必须包含可用的交互 shell、PTY 所需的 Linux 工具和
包管理/开发工具依赖；缺少 `bash`、`apt`、`dpkg`、Python、Git 或 SSH 时，构建和启动验收必须失败。

## 构建 PRoot

需要 Android NDK、Git、Make 和 POSIX shell。默认构建 `arm64-v8a`：

```bash
pnpm build:android-proot
pnpm build:android-proot -- --abi x86_64
```

脚本会从锁定 commit 构建 Termux PRoot 和静态 talloc，检查目标 ELF 的 ABI、PIE 类型、
Android linker 和动态依赖，并在目标 ABI 目录生成：

- `libagnes_proot.so`；
- `libagnes_proot_loader.so`；
- `licenses/`；
- `sources.lock.json`；
- `proot.spdx.json`。

可使用 `--work-dir` 保留构建目录，或使用 `--proot-repository`、`--talloc-repository` 指向
预先准备的本地镜像。完整参数见 `pnpm build:android-proot -- --help`。

## 构建 Runtime ZIP

准备好已安装 Python、Agent 锁定依赖和基础命令的 Ubuntu rootfs 后运行：

```bash
pnpm build:android-runtime -- --rootfs <rootfs-path> --abi <arm64-v8a|x86_64>
```

脚本会注入 Agnes Agent，生成 manifest，校验 rootfs 后输出 `runtime.zip`。打包时还会补齐
`rootfs/dev/shm` 和 `rootfs/tmp`，并将 Python 的 `pip3`/`pip` 入口规范化为可解析的真实
脚本，拒绝自指软链接。

可选传入 `--node-tarball <node-dist.tar.xz>` 把官方 Node.js dist 合并进 `usr/local`
（版本与 SHA-256 锁定在 `sources.lock.json` 的 `node` 节；脚本会用 readelf 校验二进制
架构与 `--abi` 匹配，并验证 `usr/local/bin/node` 与 `npm`/`npx`/`corepack` 可解析）。
当前 arm64-v8a 制品已预装 Node.js 22.20.0。

当前 x86_64
Ubuntu 24.04.4 完整制品已在 Android 模拟器通过首次安装、PRoot 启动、Agent Protocol 握手和
模型流式回复验证；
Ubuntu rootfs 的下载、apt 安装和锁定 Python 依赖安装目前仍是人工制备流程，不能视为完整的
可复现发布流水线。
