# Skills 系统

Agnes Skills 使用兼容主流 Agent 工具的目录格式：每个 Skill 是一个包含 `SKILL.md` 的目录，可额外携带 `references/`、`scripts/`、`assets/` 等资源。

## 格式

`SKILL.md` 必须包含 YAML frontmatter：

```markdown
---
name: document-review
description: Review documents with section-level citations.
version: 1.0.0
author: Agnes
---

# Workflow

Read `references/guide.md` before reviewing the document.
```

必填字段为 `name` 和 `description`；`version`、`author` 可选。正文是用户明确选择该 Skill 后注入本轮上下文的工作流指令。

## 安装与存储

- 本地安装：选择单个 Skill 目录，或选择包含多个 `SKILL.md` 的仓库目录批量扫描。
- Git 安装：仅接受不带内嵌凭证的 HTTPS 仓库地址，以浅克隆方式下载并扫描 Skill。
- 安装目录：`~/.agnes/skills/{skill_id}`。
- 更新：再次安装同名 Skill 时原子替换目录，保留启用/停用状态和首次安装时间。
- 卸载：移动到 `~/.agnes/skills/.trash/`，不直接永久删除。
- Skills 当前属于设备本地可执行内容，不进入结构化 D1 同步；后续跨设备分发应使用经过签名、Hash 校验和 E2EE 的大对象制品链路。

安装器拒绝符号链接、路径越界、缺失 frontmatter、空指令和超限包。单个文件最大 5 MiB，单个 Skill 最大 500 个文件、20 MiB，`SKILL.md` 指令正文最大 256 KiB。

## 使用链路

```text
设置 → Skills → 本地/Git 安装 → 校验并登记
                                   ↓
聊天输入框 → 附件 → Skills → 选择一个或多个已启用 Skill
                                   ↓
message_parts 持久化 Skill id
                                   ↓
本轮运行解析已安装版本 → 注入 Active Skills 指令层
                                   ↓
Skill 资源目录加入该轮文件读取边界 → Agent 按需读取 references/scripts/assets
```

Skill 只对附加它的用户消息及对应回复生效。编辑并重发会保留附件；重新生成会沿用原用户消息上的 Skill。停用或卸载后，不允许新的运行继续加载该 Skill。

## 安全边界

- Skill 是用户主动安装并主动选择的工作流指令，不按普通知识资料处理；但其优先级低于系统提示词、角色卡安全规则和用户当前请求。
- Skill 不能扩大 Tool Policy、权限模式、审批规则、网络策略或沙箱边界。
- 资源目录只作为选中 Skill 的读取根目录加入本轮有效策略；写入仍受 workspace 和沙箱写边界限制。
- Git 安装关闭交互式凭证请求并禁用 `file://` 协议来源，不自动递归子模块。
- 安装目录中的 `.agnes-skill.json` 由 Agnes 管理，源包不能覆盖该文件。
