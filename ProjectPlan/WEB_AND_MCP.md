# 联网研究与 MCP

> 状态：联网研究 Phase 1 已实现，MCP 待实现
> 日期：2026-07-18
> 关联文档：`PROJECT.md`、`TOOLS_AND_SANDBOX.md`

## 目标与边界

联网能力首先服务于通用生活、办公和阅读讨论中的时效性问题，不依赖工作区，也不要求用户安装额外服务。首阶段只提供只读研究能力，不包含登录、表单提交、购买、发布内容或远程控制浏览器等有副作用操作。

内置工具分为：

- `web_search`：搜索公开网页，返回标题、URL、摘要和来源；
- `web_fetch`：读取公开 HTTP/HTTPS URL，提取适合模型使用的正文文本。

搜索摘要只用于发现来源。模型在依赖网页事实作答前应读取相关页面，并在最终回答中使用实际 URL 或 `final_url` 提供 Markdown 来源链接。

## 搜索 Provider

首阶段使用无需密钥的 HTML 搜索：

1. `auto` 默认先调用 DuckDuckGo；请求失败或没有结果时回退 Bing；
2. 用户可以在 Agent 工具设置中固定 `duckduckgo` 或 `bing`；
3. Provider 只负责搜索，不绕过目标网站的访问控制；抓取失败时模型应改用其他公开来源。

该实现不把某一家搜索页面结构视为稳定 API。后续增加 Provider trait 时优先支持可配置 SearXNG 和正式搜索 API，并保留当前无密钥 Provider 作为本地默认与故障回退。

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

- 抽象 `SearchProvider`，增加可配置 SearXNG 与带密钥的正式搜索 API；
- API 密钥进入系统 Keyring，不进入 Agent JSON、消息、审计参数或同步 payload；
- 增加 Provider 健康状态、限流分类和显式回退顺序。

### Phase 3：MCP Client

- 支持本地 `stdio` 与远端 Streamable HTTP MCP server；
- MCP 工具动态映射到现有工具 schema、风险、会话权限、审批卡和审计记录；
- MCP server 按 Agent 启用，环境变量和密钥使用设备本地 secret 引用；
- server 返回内容与网页相同，默认作为不可信工具数据处理。

### Phase 4：浏览器操作

仅在只读搜索无法覆盖实际需求后增加。浏览器读取与登录/点击/提交必须使用不同能力和风险级别；涉及账户状态或外部副作用的操作不得沿用 Low 风险 `web_fetch` 权限。

## 验收标准

- 默认 Agent 可搜索公开网页、读取正文并在回答中引用来源；
- DuckDuckGo 不可用或无结果时 `auto` 能回退 Bing；
- 角色关闭 Web 或总网络后，模型看不到工具且执行端无法绕过；
- localhost、私网 IP、DNS 指向私网和重定向到私网均被拒绝；
- 超大响应、二进制响应和无正文页面以结构化错误结束，不留下 pending 消息；
- Web 工具调用完整出现在实时消息卡与工具审计中。
