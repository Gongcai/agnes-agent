# Linux 桌面发行构建

> 状态：已按 V0.1.0 实际构建验证
>
> 日期：2026-07-23
>
> 适用范围：Linux x86_64 的 `.deb`、`.rpm` 与 AppImage 发行包
>
> 关联文件：`package.json`、`scripts/build-sidecar.mjs`、`src-tauri/tauri.conf.json`

## 1. 构建目标与原则

Linux 发行包必须同时包含：

- Tauri 主程序 `agnes-agent`；
- PyInstaller 冻结的 Agent sidecar `agentd`；
- PyInstaller 冻结的文档解析 sidecar `document-parserd`；
- 512x512 应用图标和桌面入口。

发行构建不能依赖目标机器安装 Python、`uv` 或项目源码。`pnpm tauri build` 的
`beforeBuildCommand` 会先执行前端构建和 `pnpm build:sidecar`，后者负责冻结两个
sidecar，并对其执行真实协议握手冒烟测试。

以下约束必须遵守：

- 前端依赖统一使用 pnpm，不使用 npm；
- PyInstaller 不能跨平台冻结 sidecar，默认必须在目标平台本机构建；
- 不把 `src-tauri/target/`、`.sidecar-build/` 或 `.sidecar-dist/` 提交到 Git；
- AppImage 必须从完成 sidecar 冻结的同一次源码状态构建；
- 修改图标后先重建 `.deb`/`.rpm`，再构建 AppImage，避免 linuxdeploy 发现旧包中的同名图标；
- 不修改 `/usr/lib`，也不直接修改 Tauri 的用户级工具缓存来绕过发行问题。

## 2. 环境预检

构建机需要 Node.js、pnpm、Rust stable、`uv`、GTK 3、WebKitGTK 4.1、
`pkg-config`、PyInstaller 所需的本地编译工具，以及 AppImage/打包工具依赖。
完整验收还需要 `file`、`sha256sum`、`patch`、`bsdtar`、ImageMagick 的
`identify`、Xvfb 的 `xvfb-run` 和 GNU `timeout`。

```bash
node --version
pnpm --version
rustc -vV
cargo --version
uv --version
pkg-config --modversion gtk+-3.0
pkg-config --modversion webkit2gtk-4.1
```

首次构建或锁文件变化后安装依赖：

```bash
pnpm install --frozen-lockfile
```

发布前确认版本号一致：

- `package.json` 的 `version`；
- `src-tauri/tauri.conf.json` 的 `version`；
- `src-tauri/Cargo.toml` 的 `package.version`。

同时确认工作区没有意外修改：

```bash
git status --short
```

## 3. 常规发行构建

在 Tauri 官方 linuxdeploy 工具兼容的 Linux 发行版上，标准入口是：

```bash
pnpm tauri build
```

该命令会自动完成：

1. `pnpm build`：TypeScript 检查和 Vite 生产构建；
2. `pnpm build:sidecar`：冻结 `agentd` 与 `document-parserd`；
3. 对两个冻结 sidecar 执行握手冒烟测试；
4. Rust Release 编译；
5. 生成配置中 `bundle.targets = "all"` 指定的 Linux 包。

如果只需重试 bundle 阶段，必须先确认同一源码状态下的前端、Release 二进制和
两个 sidecar 已成功生成，才能显式跳过 `beforeBuildCommand`：

```bash
pnpm tauri build \
  --bundles deb rpm \
  --config '{"build":{"beforeBuildCommand":""}}'
```

不能在首次构建时直接使用这个跳过命令，否则可能把旧 sidecar 或旧前端资源装入发行包。

## 4. Arch Linux 的已验证流程

### 4.1 问题背景

截至 2026-07-22，当前 Arch Linux 构建机与 Tauri 下载的 linuxdeploy 工具存在两处兼容问题：

1. Arch 系统库包含 ELF `.relr.dyn`，旧版 linuxdeploy 内置的 `strip` 无法识别；
2. 新版 `gdk-pixbuf2` 已内置常用加载器，但 `pkg-config` 仍返回已经不存在的
   `/usr/lib/gdk-pixbuf-2.0/2.10.0` 模块目录，GTK plugin 会尝试复制该目录并失败。

`NO_STRIP=1` 只禁用 linuxdeploy 对已编译依赖的二次 strip，不会取消
`Cargo.toml` 中 Rust Release profile 的 `strip = true`。

Linux 主程序在 GTK/WebKitGTK 初始化前默认设置
`WEBKIT_DISABLE_DMABUF_RENDERER=1` 和 `GDK_BACKEND=x11`，与 `pnpm tauri dev` 的
运行环境保持一致。这规避了部分 Arch GPU/GBM 组合下窗口存在但 WebView 内容为空的问题，
也避免 Hyprland 原生 Wayland 后端与当前快速窗口行为存在差异。用户显式提供任一环境变量时
保留用户值，因此仍可用 `GDK_BACKEND=wayland` 单独验证原生 Wayland 后端。

这些是运行时兼容参数，不是 AppImage 构建参数。旧发行版或排查空白窗口时应显式使用：

```bash
env \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  GDK_BACKEND=x11 \
  "$HOME/Applications/agnes-agent_0.1.0_amd64.AppImage"
```

如果显式参数可以正常显示而直接启动为空白，必须先核对发行源码是否包含上述启动环境初始化，
不能把该现象误判为 sidecar 或前端资源打包失败。

### 4.2 先构建前端和 sidecar

先显式完成完整的构建前置步骤。任一冒烟测试失败都必须停止发布：

```bash
pnpm build
pnpm build:sidecar
```

冻结后的 Tauri sidecar 位于：

```text
src-tauri/binaries/agentd-x86_64-unknown-linux-gnu
src-tauri/binaries/document-parserd-x86_64-unknown-linux-gnu
```

### 4.3 先生成 Deb 和 RPM

```bash
pnpm tauri build \
  --bundles deb rpm \
  --config '{"build":{"beforeBuildCommand":""}}'
```

预期产物：

```text
src-tauri/target/release/bundle/deb/agnes-agent_<version>_amd64.deb
src-tauri/target/release/bundle/rpm/agnes-agent-<version>-1.x86_64.rpm
```

### 4.4 在临时缓存中修补 GTK plugin

先确保 Tauri 默认缓存已经下载以下工具。首次 AppImage 构建即使在 GTK 阶段失败，
通常也会完成这些文件的下载：

```text
${XDG_CACHE_HOME:-$HOME/.cache}/tauri/AppRun-x86_64
${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-x86_64.AppImage
${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-appimage.AppImage
${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-gstreamer.sh
${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-gtk.sh
```

复制到一次性的临时缓存，不修改用户级原文件：

```bash
AGNES_TAURI_CACHE="$(mktemp -d /tmp/agnes-tauri-cache.XXXXXX)"
mkdir -p "$AGNES_TAURI_CACHE/tauri"
cp -a "${XDG_CACHE_HOME:-$HOME/.cache}/tauri/." "$AGNES_TAURI_CACHE/tauri/"
```

在临时副本上应用补丁：

```bash
(
  cd "$AGNES_TAURI_CACHE/tauri"
  patch -p0 <<'PATCH'
--- linuxdeploy-plugin-gtk.sh
+++ linuxdeploy-plugin-gtk.sh
@@ -253,10 +253,13 @@
 gdk_pixbuf_moduledir="$(get_pkgconf_variable "gdk_pixbuf_moduledir" "gdk-pixbuf-2.0")"
 # Note: gdk_pixbuf_query_loaders variable is not defined on some systems
 gdk_pixbuf_query="$(search_tool "gdk-pixbuf-query-loaders" "gdk-pixbuf-2.0")"
-copy_tree "$gdk_pixbuf_binarydir" "$APPDIR/"
+if [ -d "$gdk_pixbuf_binarydir" ]; then
+    copy_tree "$gdk_pixbuf_binarydir" "$APPDIR/"
+fi
 cat >> "$HOOKFILE" <<EOF
 export GDK_PIXBUF_MODULE_FILE="\$APPDIR/$gdk_pixbuf_cache_file"
 EOF
+mkdir -p "$APPDIR/$(dirname "$gdk_pixbuf_cache_file")"
 if [ -x "$gdk_pixbuf_query" ]; then
     echo "Updating pixbuf cache in $APPDIR/$gdk_pixbuf_cache_file"
     "$gdk_pixbuf_query" > "$APPDIR/$gdk_pixbuf_cache_file"
PATCH
)
```

这个补丁只把已不存在的外部 loader 目录视为可选，并确保缓存文件的父目录存在；
它不会改变应用代码或系统 GTK/GDK 安装。

### 4.5 构建 AppImage

通过 `XDG_CACHE_HOME` 让本次构建只使用临时工具缓存：

```bash
env \
  XDG_CACHE_HOME="$AGNES_TAURI_CACHE" \
  NO_STRIP=1 \
  pnpm tauri build \
    --bundles appimage \
    --verbose \
    --config '{"build":{"beforeBuildCommand":""}}'
```

预期产物：

```text
src-tauri/target/release/bundle/appimage/agnes-agent_<version>_amd64.AppImage
```

成功日志必须包含 `Deploying icon .../512x512/...` 和 AppImage `Success`。
`find` 对已不存在的 gdk-pixbuf loader 目录打印警告可以接受；打包命令非零退出不可接受。

## 5. 产物校验

### 5.1 格式、大小和校验和

```bash
file \
  src-tauri/target/release/bundle/deb/*.deb \
  src-tauri/target/release/bundle/rpm/*.rpm \
  src-tauri/target/release/bundle/appimage/*.AppImage

du -h \
  src-tauri/target/release/bundle/deb/*.deb \
  src-tauri/target/release/bundle/rpm/*.rpm \
  src-tauri/target/release/bundle/appimage/*.AppImage

sha256sum \
  src-tauri/target/release/bundle/deb/*.deb \
  src-tauri/target/release/bundle/rpm/*.rpm \
  src-tauri/target/release/bundle/appimage/*.AppImage
```

每次重建都要重新生成 SHA256，不能沿用旧发行版的 Hash。

### 5.2 包内容

RPM 可直接用 `bsdtar` 检查：

```bash
bsdtar -tf src-tauri/target/release/bundle/rpm/*.rpm
```

Debian 系统可使用：

```bash
dpkg-deb --contents src-tauri/target/release/bundle/deb/*.deb
```

Arch Linux 没有 `dpkg-deb` 时，先解开 Deb 的 ar 容器，再检查 `data.tar.gz`：

```bash
AGNES_DEB_VERIFY="$(mktemp -d /tmp/agnes-deb-verify.XXXXXX)"
(
  cd "$AGNES_DEB_VERIFY"
  bsdtar -xf /absolute/path/to/agnes-agent_<version>_amd64.deb
  bsdtar -tf data.tar.gz
)
```

两个包都必须包含：

```text
usr/bin/agnes-agent
usr/bin/agentd
usr/bin/document-parserd
usr/share/applications/agnes-agent.desktop
usr/share/icons/hicolor/512x512/apps/agnes-agent.png
```

### 5.3 AppImage 解包校验

```bash
AGNES_APPIMAGE_VERIFY="$(mktemp -d /tmp/agnes-appimage-verify.XXXXXX)"
(
  cd "$AGNES_APPIMAGE_VERIFY"
  /absolute/path/to/agnes-agent_<version>_amd64.AppImage --appimage-extract
)

file \
  "$AGNES_APPIMAGE_VERIFY/squashfs-root/usr/bin/agnes-agent" \
  "$AGNES_APPIMAGE_VERIFY/squashfs-root/usr/bin/agentd" \
  "$AGNES_APPIMAGE_VERIFY/squashfs-root/usr/bin/document-parserd"

identify \
  "$AGNES_APPIMAGE_VERIFY/squashfs-root/agnes-agent.png" \
  "$AGNES_APPIMAGE_VERIFY/squashfs-root/usr/share/icons/hicolor/512x512/apps/agnes-agent.png"
```

根图标及 hicolor 图标都必须是 512x512，不能残留旧的 1x1 占位图。

### 5.4 隔离 GUI 冒烟测试

使用临时 XDG 目录，避免测试写入日常配置：

```bash
mkdir -p \
  /tmp/agnes-release-smoke/config \
  /tmp/agnes-release-smoke/data \
  /tmp/agnes-release-smoke/cache \
  /tmp/agnes-release-smoke/runtime
chmod 700 /tmp/agnes-release-smoke/runtime

timeout --signal=TERM 15s xvfb-run -a env \
  GDK_BACKEND=x11 \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  XDG_CONFIG_HOME=/tmp/agnes-release-smoke/config \
  XDG_DATA_HOME=/tmp/agnes-release-smoke/data \
  XDG_CACHE_HOME=/tmp/agnes-release-smoke/cache \
  XDG_RUNTIME_DIR=/tmp/agnes-release-smoke/runtime \
  /absolute/path/to/agnes-agent_<version>_amd64.AppImage \
  --appimage-extract-and-run
```

应用持续运行到 `timeout` 主动结束时退出码为 124，表示启动冒烟测试通过。
启动即退出、sidecar 握手失败、找不到内置二进制或 WebView 崩溃都必须阻止发布。
无真实 Wayland/DBus 会话的 Xvfb 环境可能打印 Wayland 或 user bus 警告，只要应用持续运行且
没有功能进程崩溃，这些环境警告不作为失败条件。

## 6. 发布完成条件

满足以下条件后才能把构建标记为可日常测试：

- 前端生产构建成功；
- 两个 sidecar 冻结和协议握手测试成功；
- Rust Release 构建成功；
- `.deb`、`.rpm`、AppImage 三种格式均为零退出码生成；
- 三个包都包含主程序、两个 sidecar、桌面入口和 512x512 图标；
- AppImage 隔离 GUI 冒烟测试通过；
- 记录三个文件各自的 SHA256；
- 源码提交已推送，`git status` 干净且本地分支与远程一致。
