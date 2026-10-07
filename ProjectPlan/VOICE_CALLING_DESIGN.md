# 全双工语音通话规划设计

> 状态：规划设计；ASR/TTS Provider 暂定，接口与性能须用目标百炼 Workspace 实测。
> 关联文档：VOICE_CALLING.md（可行性与竞品参考）、architecture.md（系统边界）、MODEL_ROUTING.md（模型任务分工）。

## 1. 目标与产品定义

在 Agnes 桌面端提供类似 GPT Live 的连续语音交互：用户开始语音会话后，与当前 Agent 连续对话；Agent 以语音回复；用户可随时插话，系统立即停止旧回复并处理新输入。该功能使用本机麦克风和扬声器，不接入电话号码、PSTN 或电话运营商。

首发目标是**可打断的语音 Agent**，不承诺复刻原生音频大模型的自然重叠对话、嗯哼识别与预测性接话。模型暂定如下：

| 角色 | 模型 | 示例所示接口 | 规划判断 |
|---|---|---|---|
| ASR | qwen-audio-3.1-asr-flash | ProjectPlan/qwen-audio-asr-example.txt：HTTP POST，input_audio.data 为音频 URL，format/sample_rate 指定音频格式，关闭 SSE | 目前只证明整段音频识别请求，不证明实时音频流或 partial transcript。首版按 VAD 切分 utterance 后逐段识别，由 endpointing 触发回复。 |
| TTS | cosyvoice-v3.5-flash | ProjectPlan/cosyvoice-v3.5-flash-example.py：DashScope SDK、WebSocket endpoint、SpeechSynthesizer.call() | SDK 使用 WebSocket，但示例最终返回完整音频；不能据此认定 Python 可消费首帧或在生成中追加文本。Phase 0 须检查官方流式接口和取消语义。 |

cosyvoice-v3.5-flash-voice-design-example.txt 使用的是 cosyvoice-v3.5-plus，与首选 Flash 模型不同；音色设计能力及 voice_id 兼容性不能视为已验证。CosyVoice SDK 的 `SpeechSynthesizer` 要求 `voice` 参数，第一版必须配置目标 Flash 模型可用的 `voice_id`（`DASHSCOPE_TTS_VOICE` 或应用配置），不能省略。自定义音色另行验证。

## 2. 约束与原则

1. Rust 是凭据、SQLite、session、权限与 Agent Runtime 的资源所有者；React 不直接访问系统 Keyring 或持久化语音密钥。
2. Python Agent 不直接访问数据库、文件或凭据；工具请求继续走 Rust 审批与审计流程。
3. 音频帧不作为普通消息正文或同步实体；默认不保存原始录音，只保存最终转写和最终回答。
4. 语音会话拥有独立 voice_session_id 与递增 turn_id，不能复用 run_finished 表示整个通话结束。
5. 所有流事件关联 session、turn 和序号。取消后迟到的 ASR、Agent、TTS 数据必须丢弃。
6. 桌面 MVP 优先；Android 待桌面链路稳定后验证权限、音频焦点、AEC 与后台生命周期。

## 3. 首版链路与已知限制

麦克风持续采集 → 本地 VAD 分段 → utterance 完成后提交 Qwen ASR 整段识别 → final transcript → 现有 Agent 文本流式推理 → 文本缓冲/分句 → CosyVoice 合成并播放。

这条路线支持连续会话和抢话，但 ASR 不是持续流式识别：当前示例没有证明云端 partial transcript。ASR 请求、文本 Agent 与 TTS 会串联增加首音频延迟。AI 播放期间仍持续采集并使用本地 VAD 检测新语音；插话时先本地静音和取消旧轮次，不等待 ASR final。

若 Qwen 模型另有受支持的实时音频 API，可替换 ASR adapter，不改变 Voice Session 状态机。若 CosyVoice SDK 无流式消费接口，Phase 1 可先整段合成后播放；仍能打断，但首音频延迟更高，由 Phase 0 数据决定是否接受。

## 4. 逻辑架构与职责

| 层 | 职责 |
|---|---|
| React / Tauri WebView | getUserMedia 与 AudioWorklet 采集、下采样、播放队列和立即 stop/flush；显示连接、聆听、识别、思考、播报及错误状态。 |
| Rust Core | 绑定 agent/session/voice_session；凭据读取、协议校验、生命周期与取消协调；将 final transcript 和 assistant text 写入现有消息仓储；继续托管工具审批与审计。 |
| Python Voice Session（Agent sidecar 内） | 麦克风帧缓冲、VAD/endpointing、ASR 请求调度；调用现有 Agent；文本分句、DashScope ASR/TTS adapter、turn/task cancellation；回传文本、音频和状态。 |
| 阿里云 | Qwen ASR 与 CosyVoice TTS；HTTPS/WSS，由受控运行时使用密钥。 |

声卡 I/O 在 renderer/WebView；turn 编排及云服务适配放 Python sidecar；Rust 持有会话与凭据边界、协议协调和落库。桌面端不新增独立语音守护进程。Android 若 WebView 采集/AEC 表现不足，再评估原生音频插件，但保持相同 Voice Session 协议。音频跨层使用二进制专用通道或受控分块，不用 JSON base64。Agent Protocol 保持现有 127.0.0.1 WebSocket 主从关系。

## 5. 语音状态机与抢话

### 会话状态

idle → connecting → listening ↔ recognizing ↔ thinking ↔ speaking → stopping → ended。识别、思考和播报是 UI 状态；speaking 时依旧保持采集和本地 VAD 活跃。错误可进入 recovering，失败后返回 listening 或 ended。

### Utterance 生命周期

1. 开始会话时请求麦克风权限并初始化音频设备，持续显示麦克风开启状态。
2. 音频帧送本地 VAD，同时保留 300–500 ms 前滚缓冲以免切掉首辅音。VAD 起音后收集 utterance，静音达到可调 endpointing 窗口后封段。
3. utterance 完成后创建 ASR 请求，提交 WAV/PCM。每段有大小/时长上限；超长句按安全静音点切分或提示，不能无限缓存。
4. 只有 final transcript 创建 Agent turn 并作为用户消息写入 session。空识别、低置信或服务错误不触发模型回复。
5. Agent 正文增量按短句、标点、最大等待时间与最大字符数综合分段，禁止逐 token 请求 TTS。
6. speaking 期间 VAD 检出稳定新起音后，本地立即停止播放器、清空队列、使当前 turn 失效，并取消 Agent run 和 TTS task。不得等待云 ASR 才静音。
7. 插话音频继续采集；新 utterance endpointing 后识别并创建新 turn。旧 turn 的迟到帧不能恢复播放或写入最终消息。
8. 用户结束、切换 session、权限撤销或连接失败时，取消任务、关闭连接、释放 AudioContext/track。

首发允许语音插话；“嗯/对”等 backchannel 智能忽略暂不纳入 MVP。扬声器回声可能被 ASR 当成人声，先优先耳机并验证 WebView echoCancellation；VAD 单独不能解决回声。

## 6. 协议草案

控制事件使用现有 JSON envelope 风格；音频采用二进制 WebSocket frame 或 Tauri 专用通道，不做 base64 JSON。voice_session_id 与 turn_id 由 Rust 创建/校验。协议以兼容方式扩展 Rust 真相源，并同步更新 shared/protocol.md 与 Python 镜像。

| 方向 | 事件 | 用途 |
|---|---|---|
| UI → Rust | voice_start | 开始语音并绑定 agent、session、配置引用 |
| Rust → UI | voice_started / voice_state | 确认启动和状态变化 |
| UI → Rust → Python | voice_audio_frame | 序号、采样率、声道、时间戳及 PCM 二进制帧 |
| Python → Rust → UI | voice_transcript_partial | 临时识别展示；首选 API 暂无证明，协议预留 |
| Python → Rust → UI | voice_transcript_final | 最终识别；Rust 落用户消息 |
| Python → Rust → UI | voice_assistant_delta | 当前 turn 可播报文本；避免与普通 assistant delta 重复落库 |
| Python → Rust → UI | voice_audio_chunk | 编码、序号、turn_id 与音频二进制 |
| UI → Rust → Python | voice_interrupt | 本地 VAD/用户打断；立即清播放队列 |
| Rust → Python/UI | voice_cancel_turn / voice_turn_cancelled | 取消 Agent/TTS 并确认 turn 失效 |
| UI → Rust → Python | voice_stop | 结束会话并清理资源 |
| Python → Rust → UI | voice_error | 归一化错误、阶段与可恢复性 |

帧需有最大长度、背压和有界队列；队列满时丢弃过期帧或取消 turn，禁止无界占内存。接收端先验证 session/turn 再播放。最终 DTO 由 Rust 结构体定义。

## 7. Agent 与消息落库

- ASR partial 仅临时展示，不写数据库、不同步。
- ASR final 对应一条 user 消息；assistant 文本只保存一份。TTS 是文本的音频呈现，不创建另一条消息。
- 被插话取消的 assistant 文本遵循现有 run 取消和消息持久化语义；协议开发前检查 schema 是否能标记 interrupted。不得把尚未播放的 TTS 内容伪装成用户已听到的回复。
- 键盘消息和语音消息按创建序号进入同一 session；切换 session 前先结束当前语音会话。
- 原始音频不保存；未来若增加录音，作为独立加密 artifact 和用户选项设计。

## 8. 配置与凭据

首版配置字段建议：asr_provider=aliyun_dashscope、asr_model=qwen-audio-3.1-asr-flash、tts_provider=aliyun_dashscope、tts_model=cosyvoice-v3.5-flash、region/workspace_endpoint、可选 voice、language=zh-CN、音频格式、endpointing_ms、input_limit、tts_sentence_buffer。

不要错误复用 speech_model。若纳入 Model Routing，应新增明确 ASR/TTS role，ASR 不是“文本输出模型”能力。API Key 存 Rust SecretStore，sidecar 仅在会话期间按最小必要范围使用；禁止进入 React state/localStorage、普通设置 JSON、日志、消息和同步 payload。示例 endpoint 含 Workspace ID，实际地域和 Workspace 应可配置，且与密钥分开保存。

## 9. 模块建议

建议前端新增 VoiceCallPanel、audioCapture、audioPlayback、voice protocol 模块；Rust 新增 voice 子模块，负责 commands/config/session/protocol；Python 新增 agent/app/voice/session.py、audio.py、providers/base.py、dashscope_asr.py、dashscope_tts.py。

Provider 端口应允许替换：transcribe(audio, config) → transcript；synthesize_stream(text, config) → AsyncIterator[AudioChunk]。若当前 SDK 只能整段返回，则公开 synthesize() 并声明 non-streaming，不伪造流式能力。VAD 也使用 provider-neutral 接口；先用 AudioWorklet 原型，按噪声/回声测试决定 WebRTC VAD 或平台能力，不把判断逻辑放在 React 组件中。

## 10. 分阶段计划

### Phase 0：接口与延迟 Spike

- 运行仓库内官方示例，确认 endpoint、音频格式、真实响应结构、错误映射、依赖版本和产品开通状态。
- 检查 ASR 单段长度、耗时与本地音频安全上传方式；避免将私人录音放在公开 URL。
- 查 DashScope SDK/API，确认 CosyVoice Flash 的流式消费方法、回调线程、取消方式、首音频时间、输出格式和完整耗时。
- 确认目标 Workspace、地域、凭据方式、费用和限流。
- 用至少 20 条普通话样本（静音、短句、长句、噪声）测 VAD endpoint、ASR 完成、Agent 首文本、TTS 首字节/首音频及总延迟。
- 决策门：若只能整段 TTS 且首个可听音频 p50 超过体验目标，先验证流式 API 或实时 ASR，不投入 UI 全量接线。

### Phase 1：桌面端基本往返

- 增加通话按钮、权限提示、聆听/处理/播报状态与结束控制。
- 连续采集、本地 VAD、ASR 整段识别、final transcript 注入现有 Agent、CosyVoice 播报。
- 首版可整段 TTS；若 SDK 支持真正流式消费则启用。
- 转写和 assistant 文本复用现有消息仓储，不保存音频。

### Phase 2：低延迟与可靠抢话

- 增加句子缓冲、流式 TTS 和有界播放队列。
- VAD 抢话立即静音、flush 队列、取消 Agent/TTS；迟到响应按 turn_id 丢弃。
- 增加弱网、超时、限流、重试和关闭策略，重试不能重复创建 user 消息。
- 验证扬声器回声、耳机、静音、长句与快速连续 turn。

### Phase 3：Android 与发布化

- 实测 Android 麦克风权限、AudioFocus、蓝牙、前后台生命周期、AEC 与 WebView 音频行为。
- 若需原生采集/播放，复用 Voice Session 协议与状态机。
- 完成并发上限、成本展示、隐私说明、密钥生命周期与发布态验收。

## 11. 验收目标

以下为工程目标，不是云厂商 SLA；Phase 0 后按真实数据修订。

| 指标 | MVP 目标 | 说明 |
|---|---:|---|
| 插话到扬声器静音 | p95 ≤ 150 ms | 本地 VAD/播放路径，不等待 ASR |
| endpointing 后 ASR 结果 | p50 ≤ 700 ms | 真实网络与句长测量 |
| 停说到首个可听回复 | p50 ≤ 1.5 s，p95 ≤ 2.5 s | 分开测 ASR、Agent、TTS、设备排队 |
| 旧 turn 串音/迟到播放 | 0 次 | 插话、取消、换 session 测试 |
| 意外音频持久化 | 0 处 | DB、日志、崩溃报告、同步 payload |
| 连续会话稳定性 | 20 轮无死锁/卡状态 | 桌面目标设备和网络 |

ASR 若为每 utterance HTTP 整段请求，endpointing 后 ASR 的实测延迟是首要门槛；达不到目标时应换实时 ASR 接口或接受按键式/半双工体验，不靠牺牲识别质量掩盖等待。

## 12. 风险与待确认事项

| 事项 | 当前处理 | 决策时点 |
|---|---|---|
| ASR 是否支持该模型实时输入 | 示例只证明 HTTP 整段请求，按非流式设计 | Phase 0 查正式 API 与 Workspace 权限 |
| CosyVoice Flash 流式消费/取消 | 示例 call() 返回完整音频，能力未知 | Phase 0 检查 SDK callback/iterator 与服务事件 |
| ASR 音频提交方式 | 示例使用 input_audio.data URL | 确认受控上传/本地直传，避免公开 URL |
| 格式、最大长度、地域、价格、配额 | 不写死假设 | 实测和控制台确认后固化 |
| 首发音色 | 默认/预置音色；设计示例使用 Plus | Flash 兼容性验证后再纳入 |
| 中断消息存储语义 | 遵循现有 run/message 能力 | 协议实现前检查 schema 与取消路径 |
| Android 交付范围 | Phase 3 | 桌面 MVP 验收后决定 |

## 13. 阶段完成定义

Phase 0 完成后必须有真实账户上的接口记录和延迟表；Phase 1 完成后用户能在桌面端与当前 Agent 多轮语音对话；Phase 2 完成后插话可即时静音、旧 turn 完整取消且文本历史不串轮；Phase 3 完成后 Android 通过目标设备验收，密钥与音频数据边界也经发布态检查。每阶段结束时更新本文档状态与实测结论，再进入下一阶段。
