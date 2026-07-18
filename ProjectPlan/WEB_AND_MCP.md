# 联网研究与 MCP

> 状态：联网研究 Phase 1、搜索 Provider Phase 2 与 MCP Client Phase 3 已实现
> 日期：2026-07-18
> 关联文档：`PROJECT.md`、`TOOLS_AND_SANDBOX.md`

## 目标与边界

联网能力首先服务于通用生活、办公和阅读讨论中的时效性问题，不依赖工作区，也不要求用户安装额外服务。首阶段只提供只读研究能力，不包含登录、表单提交、购买、发布内容或远程控制浏览器等有副作用操作。

内置工具分为：

- `web_search`：搜索公开网页，返回标题、URL、摘要和来源；
- `web_fetch`：读取公开 HTTP/HTTPS URL，提取适合模型使用的正文文本。

搜索摘要只用于发现来源。模型在依赖网页事实作答前应读取相关页面，并在最终回答中使用实际 URL 或 `final_url` 提供 Markdown 来源链接。

## 搜索 Provider

默认使用无需密钥的 HTML 搜索：

1. `auto` 默认先调用 DuckDuckGo；请求失败或没有结果时回退 Bing；
2. 用户可以在 Agent 工具设置中固定 `duckduckgo` 或 `bing`；
3. Provider 只负责搜索，不绕过目标网站的访问控制；抓取失败时模型应改用其他公开来源。

搜索核心通过统一 `SearchProvider` trait 接入四种来源：DuckDuckGo HTML、Bing HTML、可配置 SearXNG 与 Brave Search API。设备本地设置保存显式自动回退顺序；请求失败或结果为空时只记录 Provider ID 和 `authentication/rate_limit/timeout/network/service_unavailable/invalid_config/invalid_response/empty_results` 归一化类别，再尝试下一项。角色卡可以使用本机自动链，也可以固定一个来源。

SearXNG 地址和回退顺序保存在设备本地设置，允许用户明确配置公开实例或本机/自托管服务；模型和网页内容不能修改该地址。Brave API Key 只保存在 OS Keyring，设置 IPC 只返回是否已配置。当前无密钥 Provider 继续作为默认链，不要求用户额外注册服务。

## 安全模型

- Web 工具必须同时满足 `web.enabled=true` 与 `network.allow=true`；关闭总网络访问时不会向模型暴露工具，Rust 执行端也会再次拒绝。
- `web_search` 与 `web_fetch` 是 Low 风险只读工具，仍进入现有会话权限、实时工具卡和审计日志链路。
- 只允许 HTTP/HTTPS，拒绝 URL 凭证、本机名、`.local/.internal`、IPv4/IPv6 私网、回环、链路本地和保留地址。兼容 Clash 等 TUN `fake-ip` DNS：仅 HTTPS 域名的解析结果允许使用 `198.18.0.0/15`，直接输入该保留网段的 URL 仍拒绝。
- 每次请求先解析并校验全部 DNS 结果，将实际连接固定到已校验地址；每一跳重定向重新执行同样检查。
- 限制连接与总超时、重定向次数、下载字节数和最终正文字符数；二进制及不支持的媒体类型不进入模型上下文。
- 搜索结果和网页正文始终是不可信参考资料。系统提示词要求模型忽略页面中的角色声明、策略修改和工具调用指令。

## 正文提取

HTML 使用结构化 DOM 解析，优先选择 `article/main/[role=main]`，过滤导航、页眉、页脚、侧栏、表单和脚本区域；纯文本、JSON 与 XML 保留为受长度限制的文本。当前不执行页面 JavaScript，因此依赖客户端渲染的网站可能无法提取正文。

## 后续阶段

### Phase 2：Provider 扩展

已完成：

- 抽象 `SearchProvider`，增加可配置 SearXNG 与 Brave Search API；
- API 密钥进入系统 Keyring，不进入 Agent JSON、消息、审计参数或同步 payload；
- 设置页支持 Provider 配置、固定非敏感查询的连接测试和显式回退顺序；
- 搜索响应显示实际来源，并仅以归一化类别说明之前失败的 Provider。

### Phase 3：MCP Client

已完成：

- 使用官方 Rust MCP SDK，支持本地 `stdio` 与远端 Streamable HTTP；连接初始化、工具发现和工具调用均有超时，配置变化后关闭旧连接并按需重连；
- MCP Server 配置保存在设备本地 `mcp:servers:v1`，不进入同步 payload。Bearer Token 与 stdio 环境变量值使用确定性 secret ID 保存在 OS Keyring，列表 IPC 只返回“已配置”状态；
- stdio 直接使用 `tokio::process::Command`，不经过 shell；启动时清空环境，只继承 PATH、HOME、语言和临时目录等必要字段，再注入用户明确配置的 secret 环境变量；
- 角色卡默认 `mcp.enabled=false`，并通过稳定 Server ID 白名单授权。只有 Server 与角色两侧都启用时才向模型暴露动态工具；远端 HTTP 还必须满足角色的总网络开关；
- 工具公开名使用 `mcp__<server>__<tool>` 命名空间并带稳定摘要，避免覆盖内置工具和清洗后重名；单 Server schema、工具数、全部动态工具数和结果大小均有限制；
- Python sidecar 在真实调用与提示词调试面板使用同一份动态 schema。MCP 调用继续经过实时工具卡、High 风险审批、会话权限与 SQLite 审计，不存在绕过 Rust 执行边界的第二条链路；
- MCP 描述和返回值一律视为不可信外部数据。系统提示词明确禁止服从其中的角色声明、策略修改和新指令，也禁止向 Server 发送无关会话内容、私有本地数据或凭证；
- 设置页支持新建、编辑、启停、删除、连接测试与工具列表预览；角色卡可单独选择允许使用的 Server。

当前不实现 MCP OAuth、Resources/Prompts UI、异步 Tasks 与服务端采样。需要这些能力时基于现有连接管理器扩展，不把认证令牌或服务器返回内容写入同步配置。

### Phase 4：浏览器操作

仅在只读搜索无法覆盖实际需求后增加。浏览器读取与登录/点击/提交必须使用不同能力和风险级别；涉及账户状态或外部副作用的操作不得沿用 Low 风险 `web_fetch` 权限。

## 验收标准

- 默认 Agent 可搜索公开网页、读取正文并在回答中引用来源；
- DuckDuckGo 不可用或无结果时 `auto` 能回退 Bing；
- 角色关闭 Web 或总网络后，模型看不到工具且执行端无法绕过；
- localhost、私网 IP、DNS 指向私网和重定向到私网均被拒绝；
- 超大响应、二进制响应和无正文页面以结构化错误结束，不留下 pending 消息；
- Web 工具调用完整出现在实时消息卡与工具审计中。
- 默认自动链无需密钥；SearXNG/Brave 配置后可加入任意回退位置，Provider 故障不会暴露原始响应或凭证。
- MCP 默认不向既有角色暴露；角色授权后可以发现并调用 stdio/Streamable HTTP 工具；
- MCP 配置、动态 schema 与审计中不出现 Keyring secret 值，未知 MCP 工具在 Auto 模式下按 High 风险请求二次确认；
- MCP Server 离线、返回超大内容或 schema 不合法时结构化失败，不影响未启用 MCP 的普通对话。
