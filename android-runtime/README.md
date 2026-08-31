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

`runtime.zip` 包含 Ubuntu 用户空间和 Agnes Python Agent。`libagnes_proot.so` 是 Android
宿主侧的 PRoot 可执行文件；使用原生库命名后，Android 会把它安装到应用只读、可执行的
原生库目录。`libagnes_proot_loader.so` 是同 ABI 的静态 ELF loader，运行时通过
`PROOT_LOADER` 直接使用，避免 PRoot 从应用私有临时目录提取 loader。

Android 的 Shell 和 Git 工具会复用 Agent 启动时的 PRoot rootfs。Rust 会先完成工作区、cwd、
写入边界和审批检查，再将工作区映射为 Linux 内的 `/workspace`；工具不会获得其他 Android
应用私有目录或未授权路径。Agent 主进程和 Rust 发起的 Shell/Git/Python 工具进程都会将持久项目
根目录挂载为 `/root/workspace`，文件和记忆工具仍由 Rust 执行，Linux 进程不能绕过 Rust 审计。

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
脚本，拒绝自指软链接。当前 x86_64
Ubuntu 24.04.4 完整制品已在 Android 模拟器通过首次安装、PRoot 启动、Agent Protocol 握手和
模型流式回复验证；
Ubuntu rootfs 的下载、apt 安装和锁定 Python 依赖安装目前仍是人工制备流程，不能视为完整的
可复现发布流水线。
