# Android 内置 Ubuntu：Operit 对照研究与 Agnes 目标架构

## 1. 研究范围

本文用于指导 Agnes Android 内置 Linux Runtime 的后续设计，不是针对某个报错的补丁清单。

- OperitTerminalCore：`AAswordman/OperitTerminalCore`，commit
  `e4442bc6a047b6165bf59103721ad143149c620d`。
- 本地研究副本：`/mnt/data/Projects/OperitTerminalCore`。
- Agnes 基线：commit `40dd624` 之后的实现。
- Agnes 设备实测：2026-08-31 17:50-18:15（UTC+08）。

文中结论分为三类：

- **Operit 事实**：由指定 commit 的源码或制品直接确认。
- **Agnes 基线现状**：由设备实测和 `40dd624` 代码确认；P0 后续变化单列在 4.2 节。
- **推荐设计**：面向 Agnes 的架构决策，不代表 Operit 已这样实现。

关键参考源码见文末。行号仅对应上述固定 commit，后续上游更新可能变化。

## 2. 结论摘要

Agnes 在 `40dd624` 基线上的共同根因不是缺少 Python、pip 或某条错误提示，而是 **Android 宿主、PRoot
guest 和 Agent 工具策略没有共享同一套运行时模型**：

1. Shell 在 PRoot guest 中看到 `/workspace`、`/root` 和 `/tmp`；文件、Git 和静态检查运行在
   Android 宿主侧。此前两侧分别解析路径，导致同一目录因写法不同得到互相矛盾的结果。
2. PRoot 参数被当成固定配置，而不是设备能力探测结果。固定启用 `--link2symlink`，再叠加
   external storage 不支持 symlink/noexec，使 workspace 内的 venv 和本地可执行文件天然失败。
3. Agent 命令和用户终端不能共用同一执行会话。Agent 实际无人替它输入 sudo 密码或 apt
   提示，因此后台命令必须默认非交互、可超时、可取消；同时 Agnes 必须提供完整、独立、
   用户可直接操作的 Linux 可见 PTY，供用户运行内置 Ubuntu 工具。
4. 系统包安装仍应使用 PRoot fake-root 和无密码 sudo 兼容层；supervisor 可以提供预检、恢复和
   审计，但不能把 `apt`/`dpkg` 从普通 Linux 命令改造成只能通过 Agnes API 调用的特权功能。
5. Python、Node 和 Rust 的环境位置需要能力提示，但不能被产品强制。workspace 支持完整 POSIX
   语义时可以直接放依赖；共享存储 noexec 或缺少 symlink 时，推荐改用 app-private execution store。

Operit 已经解决了其中最重要的运行时基础问题：

- 所有 Ubuntu 进程使用 PRoot `-0`，guest 内模拟 root；
- `sudo` 是无密码参数转发 shim，不是真正的 sudo；
- `--link2symlink` 和 bind source 会先探测，缺失的 `/proc` 节点有 fake sysdata fallback；
- PRoot bind 定义与 Linux-to-host 文件路径映射共用一个模型；
- 用户可见命令走 PTY，后台执行走独立 hidden shell，并具备 marker、timeout 和进程组清理。

Agnes 应借鉴 Operit 的“可配置完整开发终端”定位，并把相同 Linux 基础能力交给用户和 Agent。
Runtime 内置或提供 Python、pip、uv、Node、fnm、Rust 等工具入口；按 workspace 使用 uv/venv、
fnm 或锁文件是推荐的依赖管理方式，不是强制隔离。Agnes 负责报告真实文件系统能力、提供可恢复
的 Runtime 和明确授权范围，不替用户或 Agent 决定全局安装还是项目环境。

## 3. Operit 的实际架构

### 3.1 制品与初始化

Operit APK 内置以下关键资源：

```text
src/main/assets/ubuntu-noble-aarch64-pd-v4.18.0.tar.xz
src/main/assets/setup_fake_sysdata.sh
src/main/jniLibs/arm64-v8a/liboperit_proot.so
src/main/jniLibs/arm64-v8a/liboperit_loader.so
src/main/jniLibs/arm64-v8a/libbash.so
src/main/jniLibs/arm64-v8a/libbusybox.so
src/main/jniLibs/arm64-v8a/libsudo.so
```

`TerminalManager` 在 app-private 目录创建 `usr/`、`bin/`、`tmp/`，再把 APK native library
目录中的二进制链接成 `proot`、`loader`、`bash`、`busybox` 和 `sudo`。Ubuntu rootfs 解压到：

```text
<filesDir>/usr/var/lib/proot-distro/installed-rootfs/ubuntu
```

安装流程使用 `ubuntu.install.lock`、`ubuntu.install.tmp` 和 `.operit_installed_ok`：正常新安装会先在
临时目录完整解压，成功后再移动到正式目录；失败会清理临时目录与锁。旧版本兼容分支只要发现
`etc/issue.net` 就会补写完成标记，因此它并非严格的完整性校验。Operit 会删除旧正式目录后再替换，
也没有 Agnes 当前 `current`/`previous` 的版本回滚。

**推荐设计**：保留 Agnes 的版本化 rootfs、摘要校验和回滚机制；吸收 Operit 的安装锁、临时目录、
完成标记与启动前 capability probe。两者并不冲突。

### 3.2 fake-root 与“sudo”

Operit 的 PRoot 启动路径和 probe 都带 `-0`。它让 guest 内的 `id`、文件属主检查以及
`apt`/`dpkg` 看到模拟 root，但 Android 内核仍以应用 UID 执行进程；它不会获得 Android root，
也无法越过 Android 应用沙箱。

`libsudo.so` 实际不是 ELF，内容只有两个字节：

```sh
$@
```

它由外层 shell 作为脚本执行，将参数原样转交。因此：

```text
sudo apt install -y jq
```

在 Operit guest 中本质上等价于：

```text
apt install -y jq
```

没有密码、提权或 sudoers。权限语义全部来自 PRoot `-0`。Operit 的初始化也直接执行
`dpkg --configure -a`、`apt install -f -y`、`apt update -y` 等命令，不依赖用户输入 sudo 密码。

这正适合手机上的 Agent 场景：用户通常无法在隐藏会话中及时补输密码。Agnes 可以提供兼容性的
no-op `sudo` shim，让已有安装脚本不因命令不存在而失败；但 Runtime API 和 Agent 提示应优先直接
执行命令，且必须明确 fake-root 不是安全边界。

### 3.3 PRoot 参数是探测结果

Operit 先用最小参数启动一次 PRoot，再逐项扩展：

1. `run_proot_probe` 先确认 `-0 -r <rootfs>` 能启动 `/bin/bash`。
2. `probe_proot_link2symlink` 单独测试 `--link2symlink`；成功后才设置
   `PROOT_LINK2SYMLINK=1`。
3. 每个 bind source 先检查存在性和 BusyBox 可访问性。
4. 候选 bind 会加入完整参数再执行一次 PRoot probe；失败则跳过，不污染最终启动参数。
5. probe 失败时输出 rootfs、loader、seccomp、link2symlink、bind 参数和原始 stdout/stderr。

因此 `--link2symlink` 在 Operit 中是可选兼容开关，不是 Ubuntu 启动的固定前提。Agnes 的
`40dd624` 基线在 Agent 启动、工具命令和 rootfs probe 三处固定加入该参数，这正是需要从根上
调整的地方。

还需注意：仅测试“PRoot 带这个参数能启动”仍不足以证明某个 bind mount 支持 venv。Agnes 的
capability probe 应同时覆盖 guest rootfs、应用私有数据目录和 workspace 三种文件系统语义。

### 3.4 bind mount 与统一路径映射

Operit 在 `PRootMountMapping` 中集中定义基础 bind：

```text
/dev
/proc
/sys
/dev/pts
/proc/self/fd   -> /dev/fd
/proc/self/fd/0 -> /dev/stdin
/proc/self/fd/1 -> /dev/stdout
/proc/self/fd/2 -> /dev/stderr
/storage/emulated/<user> -> /sdcard
/storage/emulated/<user> -> 同路径
/data/local/tmp -> /data/local/tmp
```

应用数据目录还会映射到当前 Android user 对应的 `/data/user/...`、兼容路径
`/data/data/<package>`，并保留动态 `homeDir` 映射。

关键不在 bind 数量，而在 **同一份 `PRootBindMount` 同时服务于进程启动和文件工具**。
`mapLinuxPathToHostPath()` 的规则为：

1. 规范化 Linux 路径；
2. 按 target 长度倒序匹配 bind，避免父挂载抢先命中；
3. 命中后映射回 Android source；
4. 未命中 bind 时，映射到 Ubuntu rootfs 内对应路径。

这使终端里 `/sdcard/a.txt`、文件工具里的 `/sdcard/a.txt` 和真实 Android 文件保持一致。Agnes
此前 shell 只认识 `/workspace`，宿主文件工具只认识 external storage 长路径，本质上就是缺少这一
层统一 namespace。

需要准确区分 Operit 的普通 PRoot 与 root/chroot 分支：普通 PRoot 不会获得其他应用数据权限，
`/data/user/<user>/<operit-package>` 和 `/data/data/<operit-package>` 都只映射回 Operit 自己的
`applicationInfo.dataDir`；只有显式启用、通过 `su` 启动的 chroot 分支才尝试 bind 整个
`/data/user/<user>` 和 `/data/data`。即使 mount 参数存在，最终可见范围仍由 Android UID、SELinux
和 source probe 决定，PRoot 不能提升权限。

Operit 主应用同时声明 `MANAGE_EXTERNAL_STORAGE`，Android 11+ 的权限引导通过
`ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION` 请求 All files access，并以
`Environment.isExternalStorageManager()` 判断是否授予。因此在用户允许后，`/storage/emulated/<user>`
到 `/sdcard` 的挂载确实能让 Ubuntu 命令直接遍历和修改大部分共享存储，而不只是一个无效别名。

#### 宽范围挂载的收益

1. 终端、Linux 工具、Operit 文件 Provider 和 Android 应用使用相同路径，不需要在 SAF URI、缓存
   副本与 Linux 路径之间反复导入导出。
2. 用户能直接处理 Downloads、相册、录音、现有代码仓库和其他共享文件，CLI、SSH/SFTP、编辑器、
   编译脚本之间的互操作成本低。
3. 旧脚本和工具普遍认识 `/sdcard`，保留 `/storage/emulated/<user>` 原路径又能兼容 Android 工具
   返回的绝对路径。
4. app-private data 以 Android 原路径映射后，宿主文件 API 与 guest 对同一文件的观察一致，也方便
   终端运行时组件共享 socket、脚本和状态文件。
5. 直接 bind 没有 SAF 文档树逐文件 IPC 和镜像同步成本，大目录扫描、Git、grep 与批量转换更自然。

#### 宽范围挂载的风险和限制

1. **权限爆炸半径**：只要终端进程能看到整个 `/sdcard`，用户运行的脚本、npm/pip 安装钩子、MCP
   server 或被利用的 CLI 也能读取、篡改和删除其中全部可访问文件。
2. **数据外传**：相册、下载、导出备份、聊天附件和可能误放在共享存储的密钥会同时暴露给联网
   进程；“命令由用户启动”不代表依赖代码可信。
3. **Agent 权限范围扩大**：用户选择让 Agent 与 PTY 共用宽挂载后，Agent 可以读取共享存储；这应
   被 UI 明确表达为用户授予的能力。路径别名要求策略按 canonical resource 判断，不能只检查
   `/workspace` 或 `/sdcard` 字符串。
4. **破坏性操作**：错误的 `rm`、构建清理脚本、递归格式化或恶意依赖可影响非当前项目文件，恢复
   能力取决于用户自己的备份和 Android 回收站实现。
5. **文件系统语义不完整**：emulated storage 常缺少可靠 symlink、Unix 权限和 direct exec，可能
   noexec、大小写语义不同；它适合交换文件，不适合 venv、包缓存和原生构建目录。
6. **平台与分发约束**：`MANAGE_EXTERNAL_STORAGE` 是 Android 特殊权限，用户授权摩擦高，Google
   Play 对用途有严格审核；以它作为核心路径会影响商店发布和不同 Android 版本兼容性。
7. **系统接口暴露**：`/dev`、`/proc`、`/sys` 和 `/data/local/tmp` 的宽挂载会增加设备信息与特殊
   节点暴露。Android UID/SELinux 仍会阻止多数特权操作，但“访问失败”不能代替最小挂载设计。
8. **root 模式风险质变**：普通 PRoot 下 `/data/data` 只映射本应用目录；一旦用户通过 `su` 进入
   chroot 并挂载整个 data root，恶意或误操作的影响可能扩展到其他应用数据，不再是同一级风险。

**Agnes 决策（2026-09-01）**：保留 Agent-first 产品模型，同时保证用户通过 PTY 拥有完整 Ubuntu
开发能力。文件模型向 Operit 靠近：rootfs、Agnes app-private data、guest home 和必要伪文件系统
形成统一 namespace；用户授予 All files access 后，同时映射 `/sdcard` 和 Android 原始 emulated
storage 路径。User terminal 与 Agent hidden exec 默认共享这套 Runtime 能力，让 Agent 获得接近
真机 Linux 的安装依赖、编辑文件、打开目录和运行脚本体验；用户仍可按 resource 单独撤销 Agent
访问。普通 PRoot 只映射 Agnes 自身 app data，不尝试访问其他应用私有数据。

### 3.5 Android 受限 `/proc` 的 fallback

Android 即使 bind 了 `/proc`，某些节点仍可能不存在或返回 `EACCES`。Operit 的
`setup_fake_sysdata.sh` 会在 rootfs 中生成替代数据：

```text
proc/.loadavg
proc/.stat
proc/.uptime
proc/.version
proc/.vmstat
proc/.sysctl_entry_cap_last_cap
proc/.sysctl_inotify_max_user_watches
```

启动脚本先以 `[ ! -e /proc/... ]` 判断节点缺失，再对替代文件的候选 bind 做 PRoot probe，成功后
才加入最终参数。它建立了 guest 对 Linux 系统接口的最低兼容契约，但仍有一个边界：如果节点存在、
`-e` 判断为真而实际读取返回 `EACCES`，当前逻辑不会启用 fallback。Agnes 的 probe 应直接执行读取
并记录 errno，才能覆盖设备实测的 `/proc/version Permission denied`。

Operit 的替代内容包含静态样例值，适合兼容依赖“文件存在且格式正确”的工具，不应被当成真实
设备遥测。Agnes 应在 capability profile 中标记数据来源为 `host` 或 `synthetic`，避免 Agent 把
静态 CPU、内存和 uptime 当成手机实时状态。

### 3.6 可见 PTY 与 hidden exec

Operit 明确分离两种执行平面。

**可见终端**使用 native PTY：

- `createSubprocess()` 创建 PTY master/slave；
- stdin/stdout 连接 PTY master；
- 支持窗口大小调整；
- 读取 ICANON、ECHO 等 terminal flags 和当前可读字节数；
- OutputProcessor 结合 canonical mode 与无输出状态识别 REPL、确认提示和密码等待；
- interactive mode 中的 `sendCommand()` 实际把文本和回车写入现有 PTY。

所以 Operit 可以让用户手工回答 apt 或其他交互提示。这是完整终端 UI 的能力，不代表后台 Agent
天然能回答这些问题。

**hidden exec** 则用于 Agent/后台任务：

- 按 executor key 复用一个后台 Ubuntu shell；
- 每个命令写入临时脚本，而不是直接拼接到输出解析协议；
- 通过唯一 begin/end/PID marker 收集输出和退出码；
- 使用 `setsid` 创建独立进程组；
- stdin 连接 `/dev/null`；
- 超时后向进程组发送 `SIGTERM`/`SIGKILL`，并丢弃失效 shell；
- 每个 shell 用 mutex 串行执行，避免 marker 串流互相污染。

这一设计对 Agnes 很重要。当前 Agnes 的每个 shell tool call 都创建 PTY，并通过 `write_stdin`
允许后续输入；它适合通用工具协议，却会让本应无人值守的命令误以为存在交互用户。目标设计应把
Agent 命令改为 hidden non-interactive exec，同时把用户可见 PTY 做成 Android Runtime 的一等产品
入口，而不是只在 Agent 失败时临时接管。两类会话必须拥有不同的 session namespace、生命周期
策略和 UI 状态，不能互相复用或覆盖。

### 3.7 Operit 的包管理定位

Operit 是面向用户的完整开发终端。Setup UI 允许用户选择 Python、`python3-venv`、pip、uv、
pipx、Node.js、Rust 等组件，并组合执行 apt 和安装脚本。uv 当前通过 `pipx install uv` 安装，
APT、pip、uv、npm 的镜像配置会写入 guest `/root` 或 `/etc`。

这种做法在 Operit 的产品定位下合理：用户拥有可见终端、可观察安装过程，并主动选择全局工具。
它也说明了 Agnes 不应替用户选择唯一的 Python 工作流。Runtime 只提供 Python、pip、uv 等基础
工具，并保留 PEP 668；用户和 Agent 可以自由选择全局工具、用户级安装或按项目建立虚拟环境。
需要可复现性时使用 `uv`/`venv`，需要多 Node 版本时使用 `fnm`，需要 Rust 工具链时使用
`rustup`；这些是可选能力，不是强制的 workspace 产品规则。

## 4. Agnes 实测问题的根因对照

下表以最初问题编号为索引。`40dd624` 已缓解部分现象，但“已缓解”不等于目标架构已经完成。

| 问题 | 根因 | Operit 的处理 | Agnes 目标 |
| --- | --- | --- | --- |
| A-1 `/workspace` 绝对路径被拒 | guest alias 未进入宿主策略 namespace | bind 与文件映射共用 `PRootMountMapping` | 建立唯一 mount table 和双向 path resolver；策略只判断解析后的 canonical resource |
| A-2 `~` 可写、`/root` 不可写 | `~` 在宿主 HOME 展开，guest HOME 语义丢失 | hidden/visible shell 都显式设置 guest `HOME=/root`，文件映射可落到 rootfs | 路径 API 必须携带 namespace，guest 路径只能按 guest HOME 展开 |
| A-3 `/tmp` 被静态拒绝 | 静态检查使用宿主路径语义 | guest `/tmp` 位于 rootfs/app-private 环境 | capability profile 声明 guest tmp，并映射到专用 app-private tmp；不靠文本特判 |
| A-4 `/lib`、`/lib64` 可写 | fake-root rootfs 本就由 app UID 持有，这是完整可配置 Ubuntu 的正常语义 | Operit 把 guest 当可配置终端 | 允许用户和 Agent 修改自身 rootfs；用版本回滚和审计管理风险，不用路径沙箱制造半可写 Linux |
| B 两套工具路径互不认账 | shell 与 file/git 各自解析路径 | `mapLinuxPathToHostPath()` 统一转换 | 所有工具调用同一 `GuestPathResolver`；错误报告 guest 路径及解析原因，不泄露宿主长路径 |
| C `cd` 按 token 拦截且可绕 | shell 文本扫描被误当沙箱边界 | hidden shell 本身允许正常 shell 语义 | cwd 作为执行状态维护；授权判断目标资源，静态分析只做预警/审批，不承担隔离 |
| D-1 apt 最后才失败 | guest 没有 fake-root，执行链不一致 | PRoot 固定 `-0`，sudo 为 no-op shim | 用户和 Agent shell 均可直接执行；supervisor 另提供可选预检与恢复 |
| D-2 pip 默认失败 | Ubuntu PEP 668 与“全局 pip”诉求冲突 | Operit 允许用户安装并配置 pip/pipx/uv | 保留 PEP 668，但提供清晰的 `uv`/venv 路径；不通过 Agnes 策略替用户决定是否使用全局方案 |
| D-3 workspace venv 失败 | external storage 不支持 symlink，固定 `--link2symlink` 放大冲突 | `--link2symlink` 先 probe | 提供 app-private execution store 作为推荐位置，同时允许用户选择其他可用位置并报告能力 |
| D-4 workspace noexec | Android external storage 挂载语义，不是 chmod | Operit 的工具与 rootfs 主要位于 app-private 目录 | 提供 execution store、解释器执行和能力提示；不把 noexec 伪装成权限拒绝或强制单一布局 |
| D-5 `/proc`/工具缺口 | Android Linux ABI 与系统接口不完整，rootfs 制品过小 | fake sysdata + 可选择安装工具 | 构建时定义基础 toolchain profile；启动时生成 capability profile 和 proc fallback |
| E 静态检查误伤 | 不完整 shell parser 被用作强制安全边界 | hidden exec 执行普通 shell，超时与进程组负责生命周期 | 用资源级 resolver 做路径授权；未知副作用进入审批，避免整条命令因 glob/环境读取被误拒 |
| F session ID/会话恢复 | 返回协议和 registry 不一致 | Operit session manager 与 hidden executor 各自持有 registry | 返回不可截断 opaque ID；unknown 返回当前会话；重连可查询状态，超时可靠清理进程组 |

### 4.1 对 `40dd624` 基线的评估

当前提交加入 PRoot `-0`、guest writable roots、`/workspace` 别名处理、`cd`/glob 检查修正、venv
提前提示和完整 session ID 错误。这些变化适合作为止血，并验证了问题定位，但有三处仍需架构替换：

1. Agent 启动、工具命令和 rootfs probe 仍固定使用 `--link2symlink`，尚无 capability profile。
2. 路径修复分散在 `resolve_path`、shell token 检查和 `PolicySandbox` 中，尚未形成所有工具共享的
   mount/path resolver。
3. `/etc/pip.conf` 中的 `break-system-packages = true` 会静默改变 Ubuntu 默认语义。应删除该注入、
   恢复 PEP 668，同时提供 uv/venv 和 POSIX execution store；用户明确选择的其他安装方式保持可用。

因此下一轮不应继续添加更多命令名或路径字符串特判。

### 4.2 P0 首轮实施状态

首轮实现已经完成以下基础改造：

- 新增不可变 `AndroidProotRuntimeConfig`，Agent 主进程与 shell/git 工具从同一配置生成 fake-root、
  rootfs、`/dev`、`/proc`、`/dev/shm`、workspace bind 和 guest 环境参数。
- Runtime probe 先不带 `--link2symlink` 启动真实 Python/`multiprocessing.SimpleQueue`；只有最小
  模式失败而兼容模式成功时才启用该参数，并把结果传给全部执行器。
- probe 读取 `/proc/version`、`stat`、`loadavg`、`uptime` 和 `vmstat`，记录实际可读性；当前只
  生成能力档案，synthetic proc fallback 仍属于后续工作。
- 在 rootfs、app-private environment store、Home workspace 和 app-private project workspace 中
  实测读写/rename、symlink、hardlink、direct exec，结果原子写入 `capabilities.json`，不记录宿主
  绝对路径。
- 删除安装时放开全局 pip 的行为；启动和安装会删除内容完全匹配旧 Agnes 注入值的
  `/etc/pip.conf`，其他用户配置保持不变。
- 创建持久的 `user/environments` 与 `user/cache/uv` 目录，为后续 uv environment service 准备
  execution store；本轮尚未内置 uv 或自动创建项目环境。

## 5. Agnes 推荐目标架构

### 5.1 组件边界

```text
Android UI / approval
        |
Runtime Supervisor
  |-- rootfs installer + version rollback
  |-- capability probe/profile
  |-- system provisioner (fake-root, non-interactive)
  |-- mount table + GuestPathResolver
  |-- environment manager (uv/optional conda)
  |
  |-- Agent hidden executor ------ stdin closed, timeout, process group
  |-- User visible terminal ------ PTY, resize, explicit input handoff
  |
Rust file/git/search tools
        |
same GuestPathResolver and policy resource IDs
```

Runtime Supervisor 是唯一能组装 PRoot 参数的组件。Agent manager、shell tool 和 probe 不能各自维护
一份参数列表。Supervisor 输出一个不可变的 `RuntimeLaunchSpec`，至少包含：

```text
rootfs_version
proot_path / loader_path
fake_root
link2symlink_enabled
bind_mounts[]
guest_home
guest_tmp
workspace_guest_path
environment_store_guest_path
proc_fallbacks[]
dns
capability_profile_version
```

所有执行器从该 spec 派生启动命令，所有宿主工具从同一 `bind_mounts[]` 派生路径映射。

### 5.2 capability profile

首次安装、Runtime 更新或 Android 系统版本变化后运行 probe，并持久化带版本和设备指纹的结果。
最低探测集合：

| 能力 | rootfs | app-private data | workspace |
| --- | --- | --- | --- |
| read/write/rename | 必测 | 必测 | 必测 |
| symlink/readlink | 必测 | 必测 | 必测 |
| hardlink | 必测 | 必测 | 必测 |
| chmod + direct exec | 必测 | 必测 | 必测 |
| Unix socket/FIFO | 必测 | 必测 | 可选 |
| POSIX semaphore `/dev/shm` | guest 全局探测 | - | - |
| PRoot `--link2symlink` | guest 启动及文件语义探测 | - | - |
| `/proc` 必需节点 | guest 逐节点 read probe | - | - |

probe 必须测试真实操作及 errno，不能只依据 `statfs` 或 mount 名称推测。产生的 profile 供环境管理、
错误诊断和 Agent prompt 使用，例如：

```json
{
  "workspace": {"symlink": false, "directExec": false},
  "environmentStore": {"symlink": true, "directExec": true},
  "proot": {"fakeRoot": true, "link2symlink": false},
  "proc": {"version": "synthetic", "uptime": "host"}
}
```

### 5.3 统一 guest/host 路径模型

路径值不能再只用 `String`。内部至少区分：

```text
GuestAbsolutePath
WorkspaceRelativePath
HostAbsolutePath (never exposed to the model)
```

每个 bind mount 都应有稳定 resource ID：

```text
workspace -> host:<selected workspace> -> guest:/workspace
project-root -> host:<runtime user workspace> -> guest:/root/workspace
env-store -> host:<app private envs> -> guest:/opt/agnes/envs
tmp -> host:<app private tmp> -> guest:/tmp
rootfs -> host:<version rootfs> -> guest:/
```

解析步骤为“语法规范化 -> guest HOME/cwd 展开 -> 最长 target bind 匹配 -> canonical host resource ->
策略检查”。策略记录的是 resource ID 和相对路径，不是 `/workspace` 字符串或 Android 长路径。

文件、Git、search、patch、shell cwd 和重定向审批都调用这一 API。模型只看到 guest 路径或
workspace 相对路径；EROFS、EACCES 和“策略拒绝”必须分别报告，不能混成同一个文件系统错误。

### 5.4 两类执行会话

#### Agent hidden exec（默认）

- 非 PTY，或使用明确禁用交互语义的管道；stdin 默认 `/dev/null`。
- 每个命令有唯一 marker/protocol frame、退出码、超时和输出上限。
- 每个命令建立进程组，取消或超时时清理全部后代。
- 默认设置 `CI=1`、`DEBIAN_FRONTEND=noninteractive`、`TERM=dumb` 和无颜色输出。
- 命令若检测到需要 TTY/输入，返回结构化 `requires_interactive_terminal`，不能无限等待。
- 需要保持 cwd/env 的任务可按 conversation/workspace key 复用 shell，但必须串行化并在异常后重建。

#### User visible Linux PTY（一等入口）

- 用户可以随时从 Agnes 打开内置 Ubuntu 终端；终端创建、重连和关闭都由 Runtime Service 管理。
- 每个用户终端绑定 guest HOME 和 Runtime 版本；workspace 可选，未选择项目时从 `/root` 启动。
- 支持 resize、Ctrl+C、原始字符输入和 canonical/non-canonical 状态。
- UI 清楚显示当前 cwd、命令输出、输入焦点、运行状态和 fake-root 说明。
- 终端可直接运行 shell、apt、dpkg、Python、Git、SSH 等内置 Linux 命令；除用户主动撤销的资源外，
  不添加 Agnes 命令或路径白名单，真实边界由 Android app UID、SELinux 和存储授权决定。
- 不能把隐藏 Agent session 悄悄变成等待用户输入的会话；Agent 的人工介入必须显式创建或绑定
  一个用户批准的 handoff 任务。

Agnes 可以继续保留 `write_stdin` 工具，但它只针对 Agent 管理的 interactive session。用户终端
使用独立的 terminal API 和 session registry；普通 Agent `shell` 返回 hidden exec 结果，这样
apt、构建和脚本执行不会因为分配 PTY 而改变行为。

### 5.5 apt、dpkg 与 sudo 策略

用户和 Agent 都可以像普通 PRoot Linux 一样直接运行 apt/dpkg。Runtime Supervisor 额外提供可选的
provisioning API，用于无人值守安装、失败恢复和 UI 展示。以下步骤只约束该可选 API，不限制直接
在 shell 中运行 apt/dpkg：

1. 解析明确的软件包集合，展示下载量、磁盘余量和 rootfs 版本影响；
2. 获得策略许可后，在 PRoot `-0` 环境执行；
3. 使用 `DEBIAN_FRONTEND=noninteractive`、`apt-get -y` 和配置好的 dpkg 冲突策略；
4. 记录 stdout/stderr、包清单差异和退出码；
5. 失败时执行 `dpkg --configure -a`/`apt-get -f install` 只能作为同一事务的恢复步骤；
6. 成功后更新 Runtime mutation manifest，必要时从干净版本快照回滚。

hidden executor 可以直接执行非交互 apt/dpkg，也可以选择转交 provisioning API；两条路径使用
相同 fake-root、源配置和可写 rootfs，不能因为调用者是 Agent 而拒绝系统目录写入。兼容 `sudo` 的策略为：

- guest 可内置 no-op shim，支持常见脚本；
- 不显示密码提示，也不要求用户输入密码；
- shim 不属于权限边界，包变更审计可由宿主 supervisor 记录；
- 文档和错误信息明确“root 仅在该 rootfs 内模拟”。

### 5.6 Python、Node 与 Rust：基础工具 + 可选依赖环境

Agnes 参照 Operit，交付一个可继续扩展的 Linux 基础环境，而不是把 Agent 和用户锁进预设的
workspace 工具套件。Runtime 应提供 Python 3.12、pip、uv、Node/npm 的安装入口以及
`fnm`、Rust `rustup` 等常用管理工具；用户和 Agent 可以直接使用 apt、pip、uv、npm、cargo
和各自的版本管理器。

Ubuntu 的 `EXTERNALLY-MANAGED`/PEP 668 默认保留，不偷偷写入 `break-system-packages = true`。
这不是禁止全局操作：需要全局 Python 包时，用户可以明确使用系统包、用户环境或受支持的 override；
需要项目可复现性时，可以使用 `uv venv`/`uv sync`，并把环境放到任意具备 symlink/direct-exec
能力的位置。Node 项目可用 `fnm` 切换版本并按项目管理依赖，Rust 项目可用 `rustup` 管理 toolchain。

app-private execution store（例如 `/opt/agnes/envs` 和 `/opt/agnes/cache`）是推荐位置，原因是它
通常支持完整 POSIX 语义，但不是强制位置。workspace、共享存储和其他已授权路径的能力由 probe
真实报告；当路径 noexec 或不支持 symlink 时，Agnes 应给出明确诊断和可选迁移建议，而不是把
该选择硬编码成 Agent 的工具策略。

Agent prompt 可以建议 `uv sync --locked`、`uv run`、`fnm` 或 `rustup`，但不得假设用户一定采用它们。
环境元数据、锁文件和缓存应可见、可导出、可清理；Runtime 更新不能静默删除用户自行安装的工具链。

### 5.7 workspace 与 execution store

应显式承认 Android shared/external storage 的能力限制：

| 内容 | workspace | execution store |
| --- | --- | --- |
| 源码、文档、锁文件 | 常用位置 | 也可使用 |
| Git 工作树 | 按文件系统能力 | 也可使用 |
| Python venv | 按能力和用户选择 | 推荐但非强制 |
| Node `node_modules/.bin`、原生构建产物 | 按能力和用户选择 | 推荐但非强制 |
| 下载缓存 | 按用户/工具配置 | 推荐位置之一 |
| 临时文件 | 可以使用 | `/tmp` 为默认位置 |
| 用户最终产物 | 可以使用 | 可以使用，无强制复制流程 |

对于 `./script.sh`、ELF、git hook、npm bin 或构建工具，Agnes 应报告实际的 noexec/symlink 能力，
并可建议使用 execution store 或解释器执行；但用户和 Agent 都可以自行决定迁移、重建挂载或继续
使用受限路径。错误应报告 `workspace mount does not support direct execution`，而不是建议重复 chmod。

### 5.8 安全边界

PRoot `-0` 只改变 guest 观察到的身份，不提供 Android 提权；同样，静态 shell token 扫描也不是
可靠的文件系统隔离。Agnes 应分别定义：

- Android 应用沙箱：隔离其他应用数据，是内核级边界；
- Runtime mount table：决定 guest 可见哪些宿主资源；
- Tool policy/approval：记录高风险操作并提供用户可调权限，而不是替代 Linux 的命令权限模型；
- Provisioning API：为批量或无人值守变更提供入口，同时保留用户/Agent 直接运行 apt 等命令的能力；
- 项目 environment：可选地隔离 Python/工具依赖，保证可重复性，不是强制的基础工具边界。

`/lib`、`/lib64`、`/usr`、`/etc` 和 `/var` 都属于用户拥有的可变 guest rootfs。用户和 Agent 的
Linux shell 可以修改它们，效果只限于 Agnes app UID 所拥有的 Runtime 文件，不会因此获得 Android
root。版本化基础制品、mutation manifest 和 reset/export 流程负责恢复，而不是用路径沙箱模拟只读系统。

## 6. 不应直接照搬 Operit 的部分

1. **不替用户决定依赖布局**：Agnes 与 Operit 一样提供基础 Linux 工具和宽泛选择；uv/venv、fnm、
   rustup 以及全局安装都可用，workspace 隔离只作为依赖管理选项。
2. **不依赖用户回答后台提示**：Agent 使用 hidden exec；Agnes 同时提供独立的完整可见 Linux PTY，
   但后台 Agent 不能偷偷占用或等待用户终端。
3. **不使用单份可变 rootfs 替换**：Agnes 已有版本化安装与回滚，应继续保留。
4. **不把静态 fake `/proc` 当真实遥测**：仅为格式兼容，并标记 synthetic。
5. **不把 no-op sudo 当权限控制**：权限控制在宿主 supervisor，不在 shim。
6. **不暴露 app-private 实现路径和秘密**：模型可以看到 `/root`、`/workspace`、`/sdcard` 和
   `/storage/emulated/<user>` 等真实 guest 路径，但不需要看到 rootfs 在 Android dataDir 下的实现路径，
   API Key、同步凭证、E2EE keyset 和业务数据库也不进入 Runtime namespace。

## 7. 分阶段实施建议

### P0：恢复正确默认值

- 删除全局 `break-system-packages = true` 注入，恢复 PEP 668。
- 将 `--link2symlink` 改为 probe 结果，三条启动路径统一读取同一 launch spec。
- 为 workspace、app-private env store、`/tmp` 和 `/proc` 生成 capability profile。
- 将 guest rootfs 作为用户和 Agent 均可配置的 Linux 文件系统，移除针对 `/lib`、`/etc` 等路径的
  Agnes 静态写入禁令；Android app UID 仍是外层边界。

### P1：统一 namespace

- [x] 引入首个 `GuestPathResolver`，将相对路径、`/workspace` 与
  `/root/workspace` 收敛为同一个 workspace resource ID。
- [x] resolver 内部支持按 guest target 最长前缀匹配的 mount table，后续 Runtime mount 可注入同一
  API。
- [x] file/read/write/search/patch/git/shell cwd、审批预览及 shell 静态写目标均通过
  resolver 解析；`/workspace` 不再错误依赖当前子目录 cwd。
- [ ] 将 Runtime bind mounts 显式建模为完整 mount table，并接入 rootfs、guest HOME、`/tmp`、
  Agnes runtime data、`/dev`/`/proc`/`/sys`、共享存储双路径和可选 `/data/local/tmp`。
- [ ] 建立 EROFS、EACCES、ENOENT、policy denied 的结构化错误映射。

### P2：执行平面分离

- [ ] 实现默认 hidden non-interactive executor，包含 marker、timeout、进程组清理和 shell 重建。
- [ ] 将用户可见 Linux PTY 作为独立的一等 terminal service，提供创建、输入、resize、Ctrl+C、
  输出订阅、重连、关闭和完整 session registry。
- [ ] 为 Agent hidden exec 与用户终端分配独立 session namespace；增加
  `requires_interactive_terminal` 状态及用户确认后的 UI handoff。
- [ ] 用户终端和 Agent 执行均复用 Runtime launch spec，但不能共享 stdin、输出流或取消信号。

### P3：完整 Linux 系统与可选 provisioning

- 内置 fake-root/no-op sudo 兼容层。
- 用户和 Agent 的普通 shell 均可直接运行 apt/dpkg、修改 guest 系统目录和安装工具。
- supervisor 为自动任务提供可选的非交互配置、磁盘预检、变更清单和恢复流程。
- 将 rootfs mutation 与 Agnes `current`/`previous` 版本机制关联。

### P4：基础工具与可选环境管理

- 在 Runtime 制品内置或提供 Python/pip/uv、Node/npm/fnm 和 Rust/rustup 的可靠安装入口。
- 创建支持完整 POSIX 语义的 app-private execution store，作为 venv、toolchain 和 cache 的推荐位置。
- 不强制注入 workspace 专属环境；提供环境发现、选择、清理、大小统计和重建能力。
- Miniconda/micromamba 作为可下载的可选后端，用户和 Agent 也可以自行选择其他方案。

### P5：兼容性与发布门禁

- 在 arm64 真机和 x86_64 模拟器跑相同 capability/acceptance suite。
- Runtime ZIP 构建产物记录 Ubuntu、PRoot、uv、Python 和基础工具版本及 SBOM。
- 安装、升级、回滚、磁盘不足、进程被杀和 Android 重启均加入恢复测试。

## 8. 验收标准

以下用例应成为 Android Runtime 自动化验收，而不是人工问题清单：

1. 相对路径、`/workspace/x` 和文件工具 `x` 指向同一 inode/resource。
2. `~/.cache` 与 `/root/.cache` 在 guest 语义下指向同一目录，宿主 HOME 不参与展开。
3. `/tmp` 可读写、可执行、可 symlink，且退出/重启后的清理策略明确。
4. 用户和 Agent 均可修改自身 guest rootfs 的 `/lib`、`/etc`、`/usr` 和 `/var`，但无法越过 Android app UID。
5. `apt-get install -y jq` 在用户 PTY 与 Agent hidden exec 中都能无密码完成，失败时返回真实 apt/dpkg 错误。
6. PEP 668 保持 Ubuntu 默认语义；uv/venv 可用，用户明确选择的全局或 override 方案不被 Agnes 策略拦截。
7. env 可由用户或 Agent 放在任意具备所需能力的路径；workspace noexec 时能选择 execution store 正常运行。
8. `--link2symlink` 关闭时 Runtime、uv env 和常规 Git 操作仍可工作。
9. `/proc/version` 等受限节点返回标记来源的兼容数据，不把 synthetic 值报告成真实遥测。
10. hidden exec 遇到输入等待时返回结构化状态；超时后没有后代进程或锁残留。
11. interactive PTY 可接收输入、Ctrl+C 和 resize，但不会与 hidden exec 的输出/session ID 混淆。
12. 用户可以直接打开内置 Ubuntu 终端并运行 `bash`、`apt`、`python3`、`git`，输入、resize、
    Ctrl+C 和输出订阅在重连后保持可追踪。
13. 未知 session ID 返回当前会话列表；Agent hidden exec 与用户终端的 ID、输入和输出不会混淆。
14. Runtime 更新失败后仍能启动 previous 版本，workspace、environment metadata 和用户终端状态
    按恢复策略保留或明确标记中断。
15. 同一锁文件可以重建等价环境；不同 workspace 是否共享 site-packages 由所选环境管理方式决定。
16. 授予 All files access 后，PTY、Agent shell 和文件工具对 `/sdcard` 与
    `/storage/emulated/<user>` 的映射一致，可直接编辑、搜索和运行能力允许的文件。

## 9. 源码依据

### OperitTerminalCore（固定 commit）

- `TerminalManager.kt:664-706`：native library 到 `proot`/`loader`/`bash`/`sudo` 的链接。
- `TerminalManager.kt:866-979`：最小 PRoot probe、`-0`、条件 `--link2symlink`、bind probe。
- `TerminalManager.kt:985-1084`：安装锁、临时目录、完成标记和 rootfs 替换。
- `TerminalManager.kt:1087-1108`：APT、pip、uv、npm 源配置。
- `TerminalManager.kt:1217-1258`：`/proc` fallback 与最终 PRoot 启动分支。
- `provider/filesystem/FileSystemProvider.kt:9-154`：`PRootBindMount`、mount table 和
  `mapLinuxPathToHostPath()`。
- `provider/type/LocalTerminalProvider.kt:27-136`：可见 PTY 与 hidden shell 生命周期入口。
- `provider/type/LocalTerminalProvider.kt:240-535`：marker 收集、timeout、进程组取消和临时脚本。
- `Pty.kt:42-175`：native PTY、terminal flags、可读字节数和 resize。
- `TerminalManager.kt:286-405`：普通命令、interactive input 和队列。
- `ui/SetupScreen.kt:75-100,348-430`：可选工具、apt 修复流程、uv/pipx 安装。
- `assets/setup_fake_sysdata.sh`：Android 受限 proc/sys 节点的替代数据。
- `jniLibs/arm64-v8a/libsudo.so`：两个字节 `$@` 的 sudo shim。

### Agnes P0 实现

- `src-tauri/src/agent/manager.rs`：Agent 使用共享 PRoot 配置启动，不再独立拼接能力参数。
- `src-tauri/src/tools/sandbox.rs`：共享 `AndroidProotRuntimeConfig`、guest cwd、bind 与环境变量。
- `src-tauri/src/tools/sandbox.rs:315-450`：writable roots、protected paths 与策略 sandbox。
- `src-tauri/src/tools/builtin/mod.rs:145-174`：`/workspace` 到宿主 workspace 的当前特判。
- `src-tauri/src/tools/builtin/shell.rs:80-130,340-700`：shell 静态写目标、cwd、glob 和 venv 检查。
- `src-tauri/src/tools/terminal.rs:115-330`：每命令 PTY、session registry、进程组和轮询。
- `src-tauri/src/runtime.rs:300-370`：版本化 Runtime 安装和校验。
- `src-tauri/src/runtime.rs`：Python 策略迁移、条件 PRoot probe、`/proc` 与文件系统能力档案。
- `android-runtime/README.md`：Runtime 制品、工作区和构建流程。
