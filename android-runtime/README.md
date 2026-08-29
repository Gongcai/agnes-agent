# Android Runtime 制品

此目录是 Android 应用构建时读取的本地制品入口。二进制制品不会提交到普通 Git，必须从
Ubuntu、PRoot 等上游源码独立构建。

目录结构：

```text
android-runtime/
├── arm64-v8a/
│   ├── runtime.zip
│   └── libagnes_proot.so
└── x86_64/
    ├── runtime.zip
    └── libagnes_proot.so
```

`runtime.zip` 包含 Ubuntu 用户空间和 Agnes Python Agent。`libagnes_proot.so` 是 Android
宿主侧的 PRoot 可执行文件；使用原生库命名后，Android 会把它安装到应用只读、可执行的
原生库目录。
