# Operit Android Ubuntu Runtime 实现参考

> 文档性质：外部公开实现调研，不是 Agnes 的代码来源或兼容性承诺
> 调研时间：2026-08-29
> Operit 基线：`AAswordman/Operit@f323d6c50fa661837fad06d4618462861779b562`
> Terminal Core 基线：`AAswordman/OperitTerminalCore@e4442bc6a047b6165bf59103721ad143149c620d`
> 关联文档：`ANDROID_RUNTIME.md`、`TOOLS_AND_SANDBOX.md`、`architecture.md`

## 1. 调研结论

Operit 的 Ubuntu 不是首次联网下载，而是由独立的 `OperitTerminalCore` Android library
直接打入 APK：Ubuntu 24.04 ARM64 rootfs 作为 XZ 压缩的 TAR 放在 Android assets，PRoot、
loader、BusyBox 和 Bash 作为 ARM64 原生库放在 `jniLibs/arm64-v8a`。首次使用本地终端时，
应用把压缩包复制到应用私有目录，在临时目录解压并完成基础配置，再移动为正式 rootfs。

这条交付路径证明了“安装 APK 后无需用户准备 Termux 或 Ubuntu”是可行的。Agnes 应参考其
内置资产、原生可执行文件和首次解压体验，但继续使用自己的 Runtime manifest、完整校验、
版本目录、原子指针和回滚机制，不直接复制 Operit 的代码、rootfs 或二进制制品。

## 2. 仓库与模块边界

Operit 将应用层和终端运行时拆为两个仓库：

| 模块 | 公开仓库 | 主要职责 |
|---|---|---|
| Android 主应用 | [Operit](https://github.com/AAswordman/Operit) | Agent UI、模型、工具、工作区和终端调用方 |
| Terminal Core | [OperitTerminalCore](https://github.com/AAswordman/OperitTerminalCore) | Ubuntu 资产、PRoot、PTY、会话、终端 Service、文件 Provider |

主仓库通过 Git submodule 将 Terminal Core 注册为 Gradle `:terminal` 模块。Terminal Core
既可以由主应用直接调用，也提供 `TerminalService` 和 AIDL 接口作为 Binder 边界。当前公开
Manifest 没有为该 Service 声明 `android:process`，所以默认仍与主应用同进程，不能把模块
拆分或使用 AIDL 直接等同于独立进程存活。它只声明 `arm64-v8a`，当前实现不是多 ABI Runtime。

```text
Operit Android App
  Agent / tools / workspace / UI
              │ Gradle :terminal module
              ▼
OperitTerminalCore
  TerminalManager / TerminalService / AIDL
  PTY / session / filesystem providers
              │
              ▼
Android nativeLibDir
  liboperit_proot.so / loader / busybox / bash
              │ PRoot
              ▼
App-private Ubuntu 24.04 ARM64 rootfs
```

参考入口：

- [Operit `.gitmodules`](https://github.com/AAswordman/Operit/blob/f323d6c50fa661837fad06d4618462861779b562/.gitmodules)
- [Terminal Core `build.gradle.kts`](https://github.com/AAswordman/OperitTerminalCore/blob/e4442bc6a047b6165bf59103721ad143149c620d/build.gradle.kts)
- [Terminal Core README](https://github.com/AAswordman/OperitTerminalCore/blob/e4442bc6a047b6165bf59103721ad143149c620d/README.md)

## 3. APK 内置资产

### 3.1 Ubuntu rootfs

Terminal Core 将以下文件直接提交到 `src/main/assets`：

```text
src/main/assets/
├── ubuntu-noble-aarch64-pd-v4.18.0.tar.xz
└── setup_fake_sysdata.sh
```

调研基线中的 Ubuntu 制品属性：

| 属性 | 值 |
|---|---|
| 系统 | Ubuntu 24.04.1 LTS |
| ABI | ARM64 / AArch64 |
| 压缩格式 | TAR + XZ，XZ 流带 CRC64 |
| 压缩大小 | 64,133,552 bytes，约 61.2 MiB |
| 解压大小 | 约 300.4 MiB |
| 调研时 SHA-256 | `91acaa786b8e2fbba56a9fd0f8a1188cee482b5c7baeed707b29ddaa9a294daa` |

该 rootfs 是基础 Ubuntu 用户空间，包含 Python 3 和 CA 证书等基础包；调研制品的 dpkg 状态
中没有预装 Git、OpenSSH client、Node.js、npm、tmux 或 vim。README 描述的完整开发能力
不等价于基础 rootfs 全部预装，部分能力需要后续包安装或由其他 Operit 资产提供。

### 3.2 Android 原生可执行文件

Terminal Core 将 Android 宿主侧二进制以 `.so` 名称放入原生库目录：

```text
src/main/jniLibs/arm64-v8a/
├── liboperit_proot.so
├── liboperit_loader.so
├── libbusybox.so
├── libbash.so
└── libsudo.so
```

初始化时，`TerminalManager` 从 `applicationInfo.nativeLibraryDir` 找到这些文件，在应用私有
`bin/` 中创建 `proot`、`loader`、`busybox` 和 `bash` 软链接，并确保目标具有执行权限。

这个设计不是单纯的命名技巧。较新的 Android 不应依赖从应用可写数据目录执行新写入的
ELF 文件；由 APK 原生库目录提供宿主可执行文件，兼容性和交付完整性都更可靠。Ubuntu
rootfs 中的 `/usr/bin/*` 则是 PRoot 启动后使用的 Linux 用户空间程序。

参考实现：

- [内置 Ubuntu asset](https://github.com/AAswordman/OperitTerminalCore/tree/e4442bc6a047b6165bf59103721ad143149c620d/src/main/assets)
- [ARM64 native libraries](https://github.com/AAswordman/OperitTerminalCore/tree/e4442bc6a047b6165bf59103721ad143149c620d/src/main/jniLibs/arm64-v8a)
- [`TerminalManager.linkNativeLibs`](https://github.com/AAswordman/OperitTerminalCore/blob/e4442bc6a047b6165bf59103721ad143149c620d/src/main/java/com/ai/assistance/operit/terminal/TerminalManager.kt)

## 4. 初始化与安装流程

Operit 把“准备 Android 宿主环境”和“安装 Ubuntu rootfs”分成两个阶段。

### 4.1 宿主环境初始化

`TerminalManager.initializeEnvironment()` 使用协程和互斥锁执行：

1. 创建应用私有 `usr/`、`bin/` 和 `tmp/`；
2. 为 APK 原生库目录中的 PRoot、loader、BusyBox 和 Bash 创建链接；
3. 建立 BusyBox applet 链接；
4. 把 Ubuntu TAR.XZ 和 `setup_fake_sysdata.sh` 从 assets 复制到 `filesDir`；
5. 生成 `common.sh`，写入安装、挂载、源配置和登录函数；
6. 在当前进程内把环境标记为已初始化。

大压缩包只在目标文件不存在时复制，Shell 脚本每次刷新。该阶段只准备安装输入，并未立即
展开 300 MiB rootfs。

### 4.2 Ubuntu 首次安装

首次启动本地 Shell 时，生成的 `install_ubuntu` 函数执行：

```text
filesDir/ubuntu-*.tar.xz
          │ BusyBox tar
          ▼
installed-rootfs/ubuntu.install.tmp
          │ 写 DNS、root/.bashrc、完成标记
          ▼ mv
installed-rootfs/ubuntu
          └── .operit_installed_ok
```

关键行为：

- 用目录锁和 PID 文件防止并发安装；
- 检测并清理失效锁，最长等待约 120 秒；
- 用 `ubuntu.install.tmp` 隔离未完成解压；
- 解压失败或收到退出信号时清理临时目录和锁；
- 写入 `/etc/resolv.conf` 和 `.operit_installed_ok`；
- 删除旧正式目录，再把临时目录移动到正式路径；
- 安装成功后删除 `filesDir` 中的 Ubuntu 压缩包副本，APK 内的 asset 仍然保留；
- 重置环境时删除 rootfs、临时目录、锁、启动脚本和复制出的压缩包，下次可从 APK 重新初始化。

参考实现：

- [`initializeEnvironment` 与 `extractAssets`](https://github.com/AAswordman/OperitTerminalCore/blob/e4442bc6a047b6165bf59103721ad143149c620d/src/main/java/com/ai/assistance/operit/terminal/TerminalManager.kt)
- [`CacheManager.clearCache`](https://github.com/AAswordman/OperitTerminalCore/blob/e4442bc6a047b6165bf59103721ad143149c620d/src/main/java/com/ai/assistance/operit/terminal/utils/CacheManager.kt)

## 5. PRoot 启动与文件映射

安装完成后，`common.sh` 使用宿主侧 PRoot 启动 Ubuntu：

- `-0` 在用户空间模拟 root；
- `-r <ubuntu-rootfs>` 指定根目录；
- `-w /root` 指定初始工作目录；
- 使用干净的 `env -i`，显式设置 `HOME`、`TERM`、`LANG` 和 `PATH`；
- 启动前执行最小 PRoot 探测，并按设备能力决定是否启用 `--link2symlink`；
- 动态探测 bind source 是否可访问，失败的可选挂载不加入启动参数；
- 为 Android 上缺失或不可读的部分 `/proc` 条目生成替代文件；
- 每次启动刷新 APT、pip/uv 和 npm 软件源；
- 可选 chroot 路径需要 root 权限，不是默认模式。

Operit 的终端定位决定了它会映射外部存储和应用数据目录。Agnes 的安全模型不同：Agnes
只能绑定应用私有工作区或用户通过 SAF 明确授权的目录，不能照搬其较宽的 `/sdcard`、
`/data/data` 或应用沙箱映射。

## 6. Service、会话与 IPC

Terminal Core 以 `TerminalManager` 作为单例运行时入口，管理 PTY、会话、当前目录、输出流、
SSH 连接和文件系统 Provider。`TerminalService` 再通过 AIDL 暴露受控的终端操作和事件回调，
使终端生命周期不直接依附某个 Activity。Service 为 `exported=false`，但当前没有配置独立
Android 进程；若需要跨应用进程回收后继续运行，还必须增加明确的进程与前台服务设计。

这与 Agnes 的目标分层方向一致，但协议职责不同：

| Operit | Agnes |
|---|---|
| AIDL 暴露终端会话和命令 | Tauri Android plugin 只暴露 Runtime 生命周期和窄化系统能力 |
| Agent 可以使用通用终端工具 | Rust Core 先执行 capability、审批和工作区检查 |
| Terminal Core 管理 PTY 和 rootfs | Android Runtime Service 管理 PRoot 进程，Rust Core 管理 Agent Protocol |
| Ubuntu 文件 Provider 可供 UI 浏览 | 文件能力必须经 Rust 工具层或 SAF bridge |

Agnes 不应向其他应用导出任意 Shell Binder 接口，也不应让 Python Agent 绕过 Rust 直接获得
Android 文件、SQLite、API Key 或 E2EE keyset。

## 7. 完整性、恢复与升级评价

### 7.1 Operit 已处理的可靠性问题

- APK 签名保护安装包内 assets 和 native libraries；
- XZ 流自身带 CRC64，损坏通常会导致解压失败；
- 初始化互斥和安装目录锁避免同进程/多进程重复安装；
- 临时目录避免把半解压 rootfs 直接当作正式环境；
- 失败清理和环境重置允许从 APK 重新恢复；
- PRoot 启动探测会在进入终端前暴露兼容性问题。

### 7.2 Agnes 不应照搬的缺口

- 未看到运行时对内置 rootfs 执行独立 SHA-256 manifest 校验；
- 已复制到 `filesDir` 的大压缩包只按“文件存在”复用，没有按版本和摘要验证；
- `etc/issue.net` 存在时可以补写完成标记，完整性判断较弱；
- 安装时先删除旧正式目录再移动临时目录，没有 `current/previous` 版本回滚；
- Runtime 版本写在文件名和代码常量中，不是独立、可演进的制品契约；
- 当前只支持 ARM64，不能直接满足 Agnes 的 x86_64 模拟器开发需求；
- 基础 rootfs 不等于完整 Agent 依赖环境，需要额外制品构建过程保证依赖闭包。

## 8. Agnes 采用决策

| 能力 | Operit 做法 | Agnes 决策 |
|---|---|---|
| 用户交付 | Ubuntu 随 APK assets 内置 | 采用；正式版默认内置可运行 Runtime |
| 首次安装 | 首次本地终端启动时解压 | 采用体验；应用空闲或首次进入 Runtime 时初始化 |
| 宿主 PRoot | `.so` 放入 `jniLibs` 后链接执行 | 采用；不再从可写 rootfs 路径执行 PRoot |
| rootfs 格式 | 单一 TAR.XZ 常量 | 保留可选压缩格式，但必须有 Agnes manifest |
| 完整性 | APK 签名、XZ CRC、解压结果 | 加强为 SHA-256、大小、ABI、文件数和路径校验 |
| 安装提交 | 临时目录移动到正式目录 | 采用 staging，但提交到版本目录后再切换 `current` |
| 升级恢复 | 重置后从 APK 重装 | 保留重装，并继续支持 `current/previous` 回滚 |
| ABI | 仅 `arm64-v8a` | 正式版 ARM64；开发版另提供 x86_64 Runtime |
| 文件挂载 | 较宽 Android 存储和数据路径 | 不采用；仅应用私有工作区和有效 SAF 授权 |
| IPC | AIDL 终端命令和回调 | 只参考 Service 分层；维持 Tauri plugin + Agent Protocol |
| 进程隔离 | 当前公开 Service 默认同进程 | 不假设 Service 自动隔离；Agnes supervisor 生命周期单独设计和验收 |
| 密钥边界 | 终端产品边界 | API Key、同步凭证和 E2EE keyset 永不进入 rootfs |

## 9. Agnes 后续实施清单

### 9.1 制品构建

- 从 Ubuntu 官方 ARM64/x86_64 base rootfs 独立构建；
- 安装 Python 3.12、Agent 依赖、Git、OpenSSH client、证书和首批 Linux 工具；
- 将 Agent 源码、依赖锁和启动配置固化到 Runtime 版本；
- 从 PRoot 上游为 Android ABI 构建宿主二进制；
- 生成 SBOM、第三方许可证、制品大小和 SHA-256 manifest；
- 对 rootfs 做可重复构建和启动烟雾测试。

### 9.2 Android 打包

- 将 PRoot/loader 放入 `jniLibs/<abi>`，由 Android 原生库目录提供可执行路径；
- 将 Runtime 包放入 Android assets，不放入前端 Web assets；
- ARM64 APK/AAB 只携带 ARM64 Runtime，x86_64 调试 APK 只携带 x86_64 Runtime；
- 构建时缺少目标 ABI Runtime 应明确失败，不能静默生成残缺发布包；
- 开发构建允许显式关闭内置 Runtime，继续使用手动安装测试通道。

### 9.3 首次初始化

- Android plugin 向 Rust 返回内置 Runtime asset、原生 PRoot 路径和 ABI；
- 安装前检查可用磁盘空间，并为 APK 压缩包复制和 rootfs 解压预留峰值空间；
- 复制过程使用临时文件，复制后校验包摘要；
- 调用现有 Agnes Runtime 安装器完成安全解压和版本提交；
- 安装后执行 `/usr/bin/python3`、Agent import 和 PRoot 启动探测；
- 健康检查通过后删除私有目录中的安装包副本，保留 APK asset 作为离线恢复源；
- UI 展示准备、复制、校验、解压、验证和完成状态，支持失败重试。

### 9.4 升级和恢复

- App 升级时比较内置 Runtime 版本与当前版本，不无条件覆盖用户可用版本；
- 新内置版本安装失败时继续使用当前版本；
- 启动健康检查失败可以回滚 `previous`，但要记录原因并避免无限回滚循环；
- “恢复内置 Runtime”作为高级操作，停止 Service 后从 APK 重新安装；
- 手动 ZIP 安装保留为开发、离线升级和故障恢复入口，不作为普通用户首流程。

## 10. 许可证与复用边界

Operit 和 OperitTerminalCore 的仓库主体采用 LGPL-3.0。Ubuntu rootfs、PRoot、BusyBox、Python
及预装系统包分别有自己的许可证和再分发要求。Agnes 当前采用 Apache-2.0，不能因为实现
思路相似就直接复制 Operit 的 Kotlin、Shell、rootfs 或 native libraries。

开发约束：

- 只参考公开架构、资产布局和行为；
- 从 Ubuntu、PRoot 等上游独立获取源码或制品并记录来源；
- 对每个分发文件保存版本、源码地址、许可证、补丁和校验值；
- 发布 APK/AAB 时随包提供必要版权声明、许可证文本和源码获取说明；
- 正式发布前对 PRoot、BusyBox、Ubuntu 包集合和 Python 依赖做一次许可证审计。
