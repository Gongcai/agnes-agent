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

脚本会注入 Agnes Agent，生成 manifest，校验 rootfs 后输出 `runtime.zip`。当前 x86_64
Ubuntu 24.04.4 完整制品已在 Android 模拟器通过首次安装、PRoot 启动、Agent Protocol 握手和
模型流式回复验证；
Ubuntu rootfs 的下载、apt 安装和锁定 Python 依赖安装目前仍是人工制备流程，不能视为完整的
可复现发布流水线。
