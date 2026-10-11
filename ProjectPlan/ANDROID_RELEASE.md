# Android ARM64 Release 构建与签名

## 构建

在仓库根目录执行：

```bash
pnpm android:build -- --release --apk --target aarch64 --ci
```

`--release` 由项目包装脚本处理；Tauri Android 构建本身默认使用 Release。直接执行不带参数的 `pnpm android:build` 则由包装脚本选择 Debug。

构建会校验并内置 `android-runtime/arm64-v8a/` 中的 Ubuntu Runtime ZIP、PRoot 库与 loader，然后构建前端和 ARM64 原生库。Android 发布 APK 的目标 ABI 为 `arm64-v8a`。

如果 `agent/app`、记忆提示词或 `agent/uv.lock` 已更新，先按 `ProjectPlan/ANDROID_RUNTIME.md` 重新生成 Runtime ZIP；APK 构建只暂存已有 ZIP，不会自动刷新其中的 Python Agent。输入 rootfs 的 Agent 依赖应与当前锁文件一致，原生 Python 扩展必须使用 Linux ARM64/Python 3.12 的 wheel。

包装脚本优先使用 `JAVA_HOME` 指定的完整 JDK 17 或更新版本，并在 Linux 上回退到已安装的 OpenJDK 21、17 或默认 JDK。选择时同时检查 `javac` 和 Java 版本，避免系统默认 Java 8 JRE 导致 Gradle 缺少 Java 编译器。

## 本机签名

Gradle 从 `src-tauri/gen/android/keystore.properties` 读取 `storeFile`、`storePassword`、`keyAlias` 和 `keyPassword`，使用已有的 `release` signingConfig 在打包时签名。

这些签名配置与密钥保留在本机，不纳入 Git；构建前应检查配置和密钥文件均存在。没有本机配置时，Gradle 会生成未签名产物，因此正式交付必须检查 APK 签名。

## 产物验收

使用 Android SDK Build Tools 中的工具验收最终 APK：

```bash
apksigner verify --verbose --print-certs <release.apk>
zipalign -c -P 16 -v 4 <release.apk>
aapt2 dump badging <release.apk>
sha256sum <release.apk>
```

验收内容包括签名有效性、签名证书与本机 Release 密钥一致、包名 `com.agnes.agent`、仅包含 ARM64 原生库，以及内置 Runtime ZIP 与构建输入一致。界面 Logo 与启动图标应包含最新的陶土橙字母图案。

## 2026-10-03 构建记录

- 版本：`0.1.0`，`versionCode=1000`，最低 Android API 24。
- 交付路径：`src-tauri/target/android-release/agnes-agent_0.1.0_arm64-v8a-release.apk`。
- 大小：286,335,058 字节，约 273.07 MiB。
- SHA-256：`9c190eb73df6dab3877342441eefbc0be63873a898055eede1d134b85e1d5653`。
- 本机 Release 证书 SHA-256：`4d7fae2f06b3065c4df6aa6a9272375eaba1e0d4352f1621afa28a58f6923f6a`。
- 验收通过：APK v2 签名、本机证书匹配、ZIP 对齐、ARM64 ELF 架构、内置 Runtime SHA-256，以及启动图标的透明度与显示内容。

## 2026-10-11 构建记录

- 源码提交：`f61d31f477f0be900116a30919c48569bf9cb6b8`。
- 版本：`0.1.0`，`versionCode=1000`，最低 Android API 24，目标 API 36。
- 交付路径：`src-tauri/target/android-release/agnes-agent_0.1.0_arm64-v8a-release.apk`。
- 大小：250,167,606 字节，约 238.58 MiB。
- SHA-256：`4e62c2e3bd8a0266f1b91ab047f666976dcb7f3555f343fef6db75f3c7b00237`。
- 本机 Release 证书 SHA-256：`4d7fae2f06b3065c4df6aa6a9272375eaba1e0d4352f1621afa28a58f6923f6a`。
- Runtime：`ubuntu-24.04.1-agnes-20261011-f61d31f`，已刷新 Python Agent、记忆提示词和 `agent/uv.lock` 锁定的 Linux ARM64/Python 3.12 依赖。
- Runtime ZIP 交付路径：`src-tauri/target/android-release/agnes-runtime_20261011-f61d31f_arm64-v8a.zip`。
- Runtime ZIP SHA-256：`122943394e90a48177fe212f41c7117c108d8899be39e9b6586b1c4b6553b34d`。
- 验收通过：APK v2 签名、本机证书匹配、ZIP 对齐、仅 ARM64 ELF、内置 Runtime ZIP 摘要、Runtime manifest 的 rootfs 摘要与大小，以及 16 个 Agent Python 源文件和记忆提示词与当前源码一致。
- ARM64 Python 验证：使用 QEMU 成功导入 Agent 主模块、语音提供方及锁定依赖；前端类型检查与生产构建通过。

覆盖安装 APK 会保留已安装的 Runtime。现有安装若要更新 Python Agent，请先停止 Runtime，再在设置的 Runtime 页面选择“从 ZIP 安装新版本”，导入上述新版 ZIP，然后启动。首次安装则使用 APK 内置的新版 Runtime。

APK 旁的 `SHA256SUMS`、`verification.json`、`signature-verification.txt`、`alignment-verification.txt`、`package-badging.txt` 与 `build.log` 保存产物校验信息和构建日志。`SHA256SUMS` 同时记录 APK 与 Runtime ZIP 的摘要，可在仓库根目录运行 `sha256sum -c src-tauri/target/android-release/SHA256SUMS` 验证。安装包、Runtime ZIP 与本机签名文件均不纳入 Git。
