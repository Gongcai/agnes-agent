# 文档解析

本文定义 Agnes 知识库的结构化文档解析组件、Office 与 PDF 支持范围、数据协议、安全边界和可选模型包。

## 当前支持范围

桌面端知识库当前支持：

- UTF-8 文本：Markdown、TXT、RST、LOG、CSV、JSON。
- OOXML Office：DOCX、PPTX、XLSX。
- PDF：桌面端安装可选的本地模型包后支持版面分析、表格识别和中英文 OCR。
- Google Drive 可导出的文档、演示文稿和电子表格。

第一阶段不支持：

- 旧版二进制 Office：DOC、PPT、XLS。
- 含宏格式：DOCM、PPTM、XLSM。
- 加密或密码保护的 Office 文档。

宏格式即使后续开放，也只能读取内容，绝不能执行宏。

## 组件边界

文档解析不进入现有 `agentd`。桌面端使用两个相互隔离的一次性运行时：

- `document-parserd`：随应用内置，只负责 DOCX、PPTX、XLSX，不包含 Torch。
- `docling-pdf-parserd`：位于用户显式安装的 PDF 模型包中，包含 CPU Torch 和 PDF 运行时。

```text
知识库本地/网盘导入
→ Rust 检测扩展名与 MIME
→ 文本由 Rust 内置解析器处理
→ Office 按需启动内置 document-parserd
→ PDF 按需启动已安装的 docling-pdf-parserd
→ JSONL 流式返回进度与结构化 chunks
→ Rust 校验协议与解析器指纹
→ 写入 document_versions/document_chunks/FTS
→ 后续沿用 Embedding 与加密向量制品链路
```

Office 开发态通过 `document-parser/` 独立 uv 环境运行；发布态由 PyInstaller 冻结并作为 Tauri `externalBin` 内置。PDF 开发态通过 `pdf-parser/` 的独立 uv 环境和 `.models` 运行；发布态从应用数据目录加载已校验的模型包。两类解析器完成后均退出，不常驻、不开放端口。

## Docling 接入方式

Office 固定版本：

```toml
docling-slim[format-office] == 2.113.0
pypdfium2 >= 4.30, < 5
```

Docling 2.113 的高层 `DocumentConverter` 会静态导入 PDF 模型管线和 Torch。为了保持 Office 组件轻量，当前直接调用 Docling 的声明式后端：

- `MsWordDocumentBackend`
- `MsPowerpointDocumentBackend`
- `MsExcelDocumentBackend`

这些后端仍然输出标准 `DoclingDocument`，但无需 Torch、OCR 或模型下载。

PDF 可选环境固定使用：

```toml
docling-slim[feat-ocr-rapidocr-onnx,format-pdf,models-local] == 2.113.0
torch == 2.9.1+cpu
torchvision == 0.24.1+cpu
```

PDF 管线强制 `enable_remote_services=false`、`allow_external_plugins=false`，模型只能从已安装包的 `artifacts_path` 读取。

## 解析与切块

### DOCX

- 按标题层级维护 `section_path`。
- 普通段落和列表在同一章节内聚合。
- 表格独立生成 chunk。
- 不伪造页码，`page` 保持 `null`。

DOCX 页码受字体、Office 版本和排版环境影响，不是稳定的源文档属性。需要固定页码时应先转换为 PDF，再使用后续 PDF 解析器。

### PPTX

- 每张幻灯片作为独立结构单元。
- `page` 为从 1 开始的幻灯片编号。
- 标题写入 `section_path`。
- 列表保留项目符号，表格独立生成 chunk。

### XLSX

- `page` 为 Docling 从 1 开始的工作表编号。
- `section_path` 和 `metadata.sheet` 保存工作表名称。
- 表格最多按 40 行切分，并受 1200 字符上限约束。
- 每个分块重复表头，保存 `row_start`、`row_end` 和 `header_rows`。

### PDF

- 使用 PDFium 在加载模型前检查损坏、加密状态和页数。
- 使用 Docling Layout 模型分析版面，TableFormer fast 模式识别表格。
- RapidOCR ONNX Runtime 启用中英文 OCR；内嵌文本和扫描页走同一结构化输出。
- 保留来源页码，表格继续独立切块。
- 禁用代码、公式、图片分类、图片描述和图表抽取，控制 CPU 与内存成本。

### 通用边界

- 文本块目标上限：1200 字符。
- 超长文本重叠：200 字符。
- 表格独立切块，避免与普通段落混合。
- `token_count` 当前使用保守估算，Embedding 仍按模型实际输入处理。

## 解析协议

`document-parserd` 在标准输出使用逐行 JSON（JSONL）协议。进度事件必须立即 flush：

```json
{"type":"progress","stage":"validating","percent":10,"message":"正在检查文档"}
{"type":"progress","stage":"converting","percent":45,"message":"正在解析文档结构"}
{"type":"progress","stage":"chunking","percent":80,"message":"正在生成索引分块"}
```

最终结果使用 `result` 信封：

```json
{
  "type": "result",
  "payload": {
    "schema_version": 1,
    "title": "季度报告",
    "media_type": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "source_hash": "sha256...",
    "size": 123456,
    "parser_profile": {
      "id": "docling-office-2.113.0-structured-v1",
      "name": "docling_office",
      "version": "2.113.0",
      "options_hash": "sha256..."
    },
    "chunks": [
      {
        "content": "# 系统架构\n\n- 桌面端\n- 同步服务",
        "page": 3,
        "section_path": "系统架构",
        "token_count": 12,
        "metadata": {
          "kind": "slide",
          "format": "pptx",
          "slide_number": 3
        }
      }
    ]
  }
}
```

错误使用 `{"type":"error","error":"稳定的中文错误"}`。Rust 必须限制累计输出大小，并校验事件字段、协议版本、哈希、大小、解析器指纹、chunk 内容、页码和 metadata 类型。

本地导入由前端生成 UUID 任务 ID。Rust 注册取消信号并通过 `knowledge://import-progress` 发送当前文件、阶段与百分比；取消或超时时会终止真实解析进程，并停止批量导入队列。文本导入也发送读取、切块和写入阶段的粗粒度进度。

## 数据库语义

`NewDocumentChunk` 已完整映射到：

- `page`
- `section_path`
- `token_count`
- `metadata`

文档是否需要创建新版本由以下三项共同决定：

- 源文件 SHA-256。
- `media_type`。
- `parser_profile_id`。

因此即使源文件不变，升级 Docling 或调整切块配置后也会重新解析并生成新版本。解析器配置通过 `parser_profiles` 登记，重复使用同一 ID 但内容冲突时拒绝导入。

## 安全限制

- 原文件最大 50 MiB。
- PDF 最多 2,000 页，单次解析最长 600 秒。
- OOXML 解压后总大小最大 500 MiB。
- ZIP 成员最多 20,000 个。
- PPTX 最多 1,000 张幻灯片。
- XLSX 最多 256 个工作表、2,000,000 个实际单元格。
- 拒绝异常压缩比、损坏、加密和非 OOXML 文件。
- Office 单次解析最长 180 秒，超时后终止子进程。
- 不加载外部插件，不调用远程服务，不执行宏。
- 本地与网盘导入使用同一解析和校验链路。

格式级限制在完整加载前基于 OOXML ZIP 成员和工作表 XML 流式预检，避免超大文档进入 Docling 转换阶段。

## PDF 模型包

PDF 不进入轻量 Office 二进制，而是构建为独立可选包：

```text
agnes-pdf-model-package.json
bin/docling-pdf-parserd
models/...
```

运行 `pnpm build:pdf-package` 会下载缺失的固定模型、通过 PyInstaller 冻结解析器，并生成目标平台 ZIP 和 SHA-256 文件。模型包清单记录包版本、Docling 版本、平台 target、解析器路径、模型目录及每个文件的大小和 SHA-256。

第一阶段由用户在“设置 → 模型与同步 → 本地文档模型”中选择本地 ZIP 导入，不硬编码尚未发布的下载地址，也不在首次导入时静默联网。安装过程限制 ZIP 最大 3 GiB、解压后最大 5 GiB、最多 20,000 个文件，拒绝路径穿越、符号链接、加密 ZIP、清单外文件和哈希不匹配；校验完成后原子替换旧版本。移除操作只删除应用数据目录中的已安装包。

安卓端不本地运行 Docling。桌面端完成解析后，同步 chunks 和指纹匹配的加密向量制品；安卓直接导入 PDF 后续走远端解析或轻量文本提取。
