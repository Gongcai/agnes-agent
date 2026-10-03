# Android ARM64 Release 构建与签名

## 构建

在仓库根目录执行：

```bash
pnpm android:build -- --release --apk --target aarch64 --ci
```

`--release` 由项目包装脚本处理；Tauri Android 构建本身默认使用 Release。直接执行不带参数的 `pnpm android:build` 则由包装脚本选择 Debug。

构建会校验并内置 `android-runtime/arm64-v8a/` 中的 Ubuntu Runtime ZIP、PRoot 库与 loader，然后构建前端和 ARM64 原生库。Android 发布 APK 的目标 ABI 为 `arm64-v8a`。

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

APK 旁的 `SHA256SUMS`、`verification.json` 与 `signature-verification.txt` 保存产物校验信息。安装包与本机签名文件均不纳入 Git。
