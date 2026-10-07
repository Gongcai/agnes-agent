# 全双工语音 AI 通话可行性

## 结论

在 Agnes 中实现类似 GPT Live 的连续语音对话**技术上可行**，且可以先用阿里云 ASR/TTS 作为语音前后端。这里的“通话”是客户端麦克风与扬声器上的实时语音会话，不连接 PSTN、手机号或传统电话服务。

但要区分两种体验：

- **流式语音往返**：边录音边送 ASR；识别结束后调用现有文本 Agent；文本增量送流式 TTS。响应可以较快开始，且用户能够打断。阿里云服务与当前架构能支持这条路线。
- **原生实时语音模型**：模型同时理解音频、识别话轮、生成语音，并能自然处理嗯哼、重叠说话、停顿续说等。单独拼接 ASR + 文本 LLM + TTS 不会自动得到 GPT Live 的全部交互质量；需要自行实现 VAD、endpointing、回声处理、抢话策略和播放队列。

因此建议把目标定义为“基于云 ASR/TTS 的可打断实时语音 Agent”，以桌面端先行。它能做到使用者讲话时识别继续运行、AI 播放时接受插话、插话后取消旧回复；不承诺复现 SpeakRail 的专用 turn-taking 模型或 GPT Live 原生音频模型的对话行为。

## 本项目的适配情况

现有代码适合作为 Agent 与会话底座，但不是现成的语音通话管线：

- Tauri UI → Rust Core → Python sidecar 的本地 WebSocket 主从架构已存在；Agent 文本输出通过 `assistant_delta` 流式回传。
- Rust 现有 WS 协议以 `run_request / assistant_delta / run_finished` 为轮次式文本协议，没有音频帧、语音会话或双工媒体状态。
- `speech_model` 是“语音理解模型”的文本模型分工，不是 ASR 配置，也不是 TTS 配置。模型供应商凭据可由 Rust 凭据层管理，但需扩展独立语音 Provider 配置。
- UI 当前没有麦克风采集、实时播放、设备选择和通话状态能力。桌面浏览器 WebView 可用 Web Audio / getUserMedia 作为首版采集播放路径；Android 还需要单独验收麦克风权限、AudioTrack/AudioRecord 与 Tauri WebView 行为。

主要新增工作在 Voice Session、流式协议、取消语义、TTS 队列和客户端音频 I/O，而不是重写 LangGraph 或 Agent 工具系统。

## 阿里云接口适配

建议将两个产品线分别封装成 Provider adapter，不在 UI 或 LangGraph 中直接耦合厂商请求格式。

| 能力 | 阿里云方向 | 接口形态与适配判断 | 关键限制/验证项 |
|---|---|---|---|
| 流式 ASR | 智能语音交互实时语音识别（Paraformer 实时识别系列） | 长连接实时识别，客户端连续推送音频流并接收中间/最终识别结果。适合作为常开输入通道。 | 确认选定地域、音频编码/采样率、鉴权 token 生命周期、并发与计费；识别的 partial 不能直接作为 Agent 正式用户消息。需基于正式结果和 endpointing 决定何时发起回复。 |
| 流式 TTS | 百炼 CosyVoice WebSocket 实时合成 | WebSocket 任务交互：建立连接、启动合成任务、发送文本、接收音频事件，最后结束任务。可以把 Agent 文本分段后增量合成并播放。 | 文档需逐项核对当前模型、地域、音频格式、首包延迟、分段连续性和任务结束语义。即使 WebSocket 持续输出，也不能假设文本生成可以无限追加或当前语音必然支持随时 cancel；插话时应立即关停本地播放并结束/丢弃对应 TTS task。 |
| 单体实时语音备选 | 百炼 Qwen-Omni-Realtime（若目标地域/账号开放） | 若有可用的实时双向音频 WebSocket 模型，可减少 ASR→LLM→TTS 串接层次，作为单独的 VoiceProvider 试验。 | 产品、模型和地域可用性以账号实际开通和当前文档为准；不能把它当作 LiteLLM 文本聊天 API。需验证函数调用、Agnes 工具审批、session 上下文与取消的接入方式。 |

以上阿里接口类别可由官方文档确认；具体模型 ID、地域 endpoint、请求事件和价格会变更，应在实现前以账户开通区域的现行 API 参考和控制台为准。本次研究未用阿里云密钥做在线请求，也未对端到端延迟或价格进行实测。

官方文档入口：

- [智能语音交互：实时语音识别](https://help.aliyun.com/zh/isi/user-guide/real-time-speech-recognition/)
- [百炼：CosyVoice WebSocket API 参考](https://help.aliyun.com/zh/model-studio/cosyvoice-websocket-api)
- [百炼：Qwen-Omni-Realtime API（需核实账号/地域可用性）](https://help.aliyun.com/zh/model-studio/qwen-omni-realtime-api)

## SpeakRail 路线参考

[SpeakRail README](https://github.com/speakrail/speakrail) 展示的链路是：

```text
麦克风 → Voxtral Realtime ASR + turn-taking head
       → 专用 turn-taking LLM 决策 speak/listen/interrupt/yield
       → Gemma 对话模型 → Breeze 流式 TTS → 扬声器
```

它的全双工特性来自 ASR 隐状态上的话轮分类头、专门微调的决策模型、提前 drain/peek 解码和客户端回声消除，不是简单由“同时开着 ASR 与 TTS”得到。README 报告约 0.7–0.8 秒 median 的停说到听见回复延迟，但这是它特定硬件、模型和评测条件下的数据，不能外推到云端阿里组合。它要求 Linux/NVIDIA 24 GB 显存级机器，英语优先；其 Breeze TTS 权重仅研究/非商业用途。代码 Apache-2.0 可供借鉴，模型权重需分别核查许可证。

适合 Agnes 借鉴的设计：ASR 与对话并行处理、独立话轮状态机、插话时 turn_id 作废、显示识别与决策调试状态、测量“停止说话到首个可听音频”的端到端延迟。第一版不建议移植其 GPU 推理栈或专用模型训练路线。

## 推荐架构

```text
Tauri UI（麦克风 / 扬声器 / VAD 状态）
   │ PCM 帧、序号、时间戳；二进制帧与控制事件分离
   ▼
Rust Core（权限、Keyring、session 绑定、审计、取消协调）
   │ 本地 WS 语音事件
   ▼
Python Voice Session（独立于普通 run 生命周期）
   ├── 阿里实时 ASR ── partial/final ── endpointing
   ├── Agnes Agent/LangGraph ──文本增量──┐
   ├── barge-in / turn_id / task cancellation│
   └── 阿里 CosyVoice 流式 TTS ◄───────────┘
            │ 音频帧与 turn_id
            ▼
       播放队列（过期 turn 立即丢弃）
```

职责边界建议：Tauri UI 负责采集播放、权限、静音和通话展示；Rust 负责本地 WS 接口、凭据和 session/权限绑定；Python Voice Session 负责 ASR/TTS WebSocket、turn 状态机、LLM 输出分句、音频回传以及异步取消。不要把音频帧写进 SQLite 普通消息，也不要把二进制帧塞进 LiteLLM。每轮只落库 final transcript 与 assistant 文本；音频录制默认关闭。

在本机到云端链路中，阿里凭据不能下发 renderer。优先沿用 Rust OS Keyring / Android Keystore 管理语音密钥，由受控本地服务取得短期凭证或由 Rust 代理云端 WS 握手。若客户端直连厂商需要临时 token，token 必须短时、最小权限，且绝不写入同步数据或日志。

## 话轮和打断设计

全双工首先是行为状态机，不只是全双工传输：

1. ASR 始终收音并持续报告 partial；partial 只更新 UI/临时识别上下文，不立即写正式消息。
2. VAD/ASR endpointing 判断停顿和 utterance final。用户思考中的短停顿不能轻易触发回复；可设置可调 endpointing 窗口，先保证不抢话。
3. final transcript 创建新的 `turn_id`，进入现有 Agent session；Agent 正文增量按短句/标点送 TTS，避免一个 token 一个请求。
4. TTS 音频增量附带 `turn_id` 与序号进入有界播放队列。ASR/VAD 检测到新的人声时，立刻停止播放器、清空旧队列、取消旧 Agent 运行和旧 TTS 任务。
5. 只有当前 turn_id 可播放或提交完成状态。网络晚到的旧 ASR/TTS/LLM 帧一律丢弃；取消必须端到端传播。

自身扬声器回灌会让 ASR 把 AI 声音当作用户。首版应优先耳机，并测试 WebView/平台的 echoCancellation；扬声器场景需系统 AEC 或 acoustic echo cancellation，单靠 VAD 不能解决回声。用户说“嗯”“对”等 backchannel 是否打断可后续增加策略，首版用更保守的阈值避免误切。

## 迭代计划与验收指标

| 阶段 | 内容 | 验收点 |
|---|---|---|
| 0：服务验证 | 建立阿里实时 ASR 与 CosyVoice 最小 WebSocket 客户端，记录事件、编码、费用和取消行为 | 账号地域可用；并发收发稳定；确定端点/模型/凭证；测 20 次普通话输入的 partial、final、TTS 首帧与首音频延迟 |
| 1：桌面语音往返 | Web Audio 采集 PCM，ASR final 后调用现有 Agent，分句流式 TTS 回放 | 一轮结束能写文本消息；UI 可挂断/静音；无二进制音频落库 |
| 2：可打断 | 常开 ASR、VAD/endpointing、播放队列与 turn_id 取消 | AI 讲话时插话，播放在 150 ms 内静音；旧轮次不再播、不污染后续消息；连续 20 轮无死锁/串音 |
| 3：安卓与健壮性 | 麦克风权限、前后台生命周期、蓝牙/耳机、断线恢复、限流与用量 | 目标安卓设备实测；弱网/断网可恢复或清楚失败；密钥不进入 renderer、日志和同步 payload |

建议目标是耳机环境下从用户停说到首个可听回复 p50 ≤ 1.2 秒，p95 ≤ 2.0 秒；插话到扬声器静音 ≤ 150 ms。把 ASR final、Agent 首个可播报正文、TTS 首帧和实际播放时间分开埋点，不能只测云 API 耗时。这里是工程目标，需基于真实网络和服务计划验证，不是阿里云 SLA。

## 主要风险与决策

- **云 ASR/TTS 不是原生多模态语音模型**：会有 ASR final/endpointing 边界，插话与自然停顿策略需本地实现。
- **多段 TTS 韵律**：按文本片段多次创建合成任务可能造成音色或韵律接缝；需实测 CosyVoice 支持的连续任务语义，按短句合成并保持单会话/任务。
- **取消粒度未知**：即使结束 TTS 任务，已在途音频仍可能到达；客户端 turn_id 丢弃是必须的。
- **地域、价格和限流**：智能语音交互与百炼可能是独立产品、密钥、计费和 endpoint；不能因为都是阿里云就假定一个 Key 或统一配额。
- **移动端回声与后台音频**：Android 音频焦点、AEC 和休眠策略需要真机阶段处理。
- **工具延迟**：工具审批或长工具运行会产生沉默，应以语音提示状态；审批仍使用现有 Rust 链路。
- **隐私**：音频实时上传到云侧处理；UI 应清楚指示麦克风状态并提供立即挂断。默认不保存原始录音。

推荐决策：先做一次不改产品代码的云服务验证，确认目标地域的阿里实时 ASR + CosyVoice 账户与价格；若接口实测满足首音频目标，再按上述 Voice Session 分层实现桌面 MVP。把 Qwen-Omni-Realtime 作为并行对照实验：若它账户可用且支持所需工具接入和取消能力，可能比拼接 ASR/TTS 更接近 GPT Live。
