# Logo 与应用图标

项目使用用户提供的陶土橙色字母图案，主色为 `#CC785C`，图案空白区域保持透明。

- `public/logo.png`：界面与 README 使用的透明 Logo，裁去多余空白后等比居中。
- `src-tauri/icons/icon-source.png`：应用图标源图，透明画布保留图标留白。
- `src-tauri/icons/icon-manifest.json`：Tauri 图标生成配置；安卓自适应图标使用暖白 `#FAF9F5` 背景与缩小的透明前景，避免系统遮罩裁切。
- `src-tauri/icons/`：Linux、Windows、macOS 与 iOS 的生成资源。
- `src-tauri/gen/android/app/src/main/res/mipmap-*`：安卓启动图标与自适应前景。

更新源图后执行 `pnpm icons:generate`，生成桌面图标、浏览器图标和移动端资源。Android 工程存在时，Tauri 会直接更新该工程内的图标；尚未初始化时，资源生成到 `src-tauri/icons/android/`。

安卓工程目录默认被 Git 忽略；提交新增启动图标时，应显式加入所需的 `mipmap-*` 图标和 `values/ic_launcher_background.xml` 颜色资源。

界面中的项目 Logo 由 `AppLogo` 组件显示，保留原始颜色与比例，并设置明确的显示尺寸。项目品牌图标与用户自定义的角色头像分别管理。
