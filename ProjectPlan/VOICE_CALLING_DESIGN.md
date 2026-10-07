# 全双工语音通话规划设计

> 状态：Phase 1 基础链路已接通；Phase 2 已加入手动打断、Agent/TTS 联动取消、run ID 过滤、句尾播放期间合成下一句，以及识别／思考／播报状态。采集和 VAD 已优先迁移 AudioWorklet，增加初始化、ASR 和 TTS 阶段的超时收尾。当前为防回声采用轮流收听与播报；可靠语音插话、专用语音话轮协议、ASR 二进制通道和 Android 验收仍待完成。构建检查不能替代真实设备上的连续通话与延迟实测。
> 关联文档：VOICE_CALLING.md（可行性与竞品参考）、architecture.md（系统边界）、MODEL_ROUTING.md（模型任务分工）。

## 1. 目标与产品定义

在 Agnes 桌面端提供类似 GPT Live 的连续语音交互：用户开始语音会话后，与当前 Agent 连续对话；Agent 以语音回复；用户可随时插话，系统立即停止旧回复并处理新输入。该功能使用本机麦克风和扬声器，不接入电话号码、PSTN 或电话运营商。

首发目标是**可打断的语音 Agent**，不承诺复刻原生音频大模型的自然重叠对话、嗯哼识别与预测性接话。模型暂定如下：

| 角色 | 模型 | 示例所示接口 | 规划判断 |
|---|---|---|---|
| ASR | qwen-audio-3.1-asr-flash | ProjectPlan/qwen-audio-asr-example.txt：HTTP POST，input_audio.data 为音频 URL，format/sample_rate 指定音频格式，关闭 SSE | 目前只证明整段音频识别请求，不证明实时音频流或 partial transcript。首版按 VAD 切分 utterance 后逐段识别，由 endpointing 触发回复。 |
| TTS | cosyvoice-v3.5-flash | DashScope Python SDK `SpeechSynthesizer` callback / `streaming_call()` | 官方示例使用 PCM_22050HZ_MONO_16BIT callback；当前以 PCM 块经 sidecar、Rust 事件传至 WebView，前端 Web Audio 逐块调度。ACK 限制最多一个跨层未确认块，Provider callback 桥接队列上限为 3 块。Agent 文本仍按标点分句请求独立流，不是同一 CosyVoice 请求的文本增量输入。 |

cosyvoice-v3.5-flash-voice-design-example.txt 使用的是 cosyvoice-v3.5-plus；但官方 `voice-design.md` 的支持范围确认 CosyVoice Voice Design 可直接指定 `cosyvoice-v3.5-flash`。CosyVoice SDK 的 `SpeechSynthesizer` 要求 `voice` 参数，第一版使用 `agent/app/voice/design_voice.py` 生成并配置 Flash 专属 `voice_id`（`DASHSCOPE_TTS_VOICE` 或应用配置），不能复用 v3 内置音色。具体声音描述和创建步骤见 `ProjectPlan/AGNES_VOICE_DESIGN.md`。

## 2. 约束与原则

1. Rust 是凭据、SQLite、session、权限与 Agent Runtime 的资源所有者；React 不直接访问系统 Keyring 或持久化语音密钥。
2. Python Agent 不直接访问数据库、文件或凭据；工具请求继续走 Rust 审批与审计流程。
3. 音频帧不作为普通消息正文或同步实体；默认不保存原始录音，只保存最终转写和最终回答。
4. 语音会话拥有独立 voice_session_id 与递增 turn_id，不能复用 run_finished 表示整个通话结束。
5. 所有流事件关联 session、turn 和序号。取消后迟到的 ASR、Agent、TTS 数据必须丢弃。
6. 桌面 MVP 优先；Android 待桌面链路稳定后验证权限、音频焦点、AEC 与后台生命周期。

## 3. 首版链路与已知限制

当前已落地链路：麦克风采集 → AudioWorklet 本地 RMS endpointing 分段（兼容环境使用 ScriptProcessor）→ WAV 16 kHz 转换 → Rust 受控 Tauri 命令 → 已认证 sidecar ASR 请求 → Qwen 整段识别 → 复用现有文本 Agent → assistant 增量按标点分句 → CosyVoice PCM callback → sidecar/Rust 有序事件 → Web Audio PCM 播放。播放缓冲阈值为约 800 ms，单块调度可在此基础上增加一块音频时长；sidecar 等待逐块 ACK，Provider callback 队列最多 3 块。前端在发送 ACK 前推进序号，并串行调度音频块，避免快速回传导致下一块被误丢弃。

新会话首轮发送时，Store 先取得新建会话及会话列表，再一次性发布列表、activeSessionId 和清空草稿，避免 App 在两步之间判定无可见会话而卸载 ChatWorkspace。VoiceCallPanel 的事件监听在通话期间保持注册，通过 ref 读取草稿升级后的正式 sessionId；标题更新只更新显示数据，不重建采集和播放链路。切换到另一个会话时结束当前通话。

这条路线支持连续会话和手动打断；ASR 仍为整段识别，没有云端 partial transcript。为防止 Arch Linux / WebKitGTK 扬声器回声被识别成用户输入，识别、思考和播报期间屏蔽 PCM 收集与 VAD；整轮 Agent 完成且播放队列排空后再等待 220 ms，清空残留缓冲并恢复聆听。浏览器 echoCancellation、noiseSuppression 和 autoGainControl 保留，但尚不足以证明可靠全双工。

手动打断通过按钮或 Esc 先停止本地播放、清空句子与音频队列，再取消当前 CosyVoice 请求与 Agent run。语音发送前分配唯一 run ID，Rust 校验并沿用该 ID；面板只接受本次发送对应的事件。Rust 在取消标记期间丢弃后续 assistant delta，结束旧 run 时只清除匹配的 session→run 映射。Store 的终态与消息重载也校验 run ID，避免旧结束事件影响新回复。识别期间可打断并丢弃迟到结果，但尚不能取消云端 ASR 请求。结束通话会撤销尚未发送的语音文本；若原生命令仍在准备 run，则命令返回后按返回的 run ID 精确取消，并在准备结束前阻止新消息发送。

每个完整句子仍启动独立 CosyVoice 请求；下一句会在上一句的已缓冲尾音播放期间开始合成。若取消先于 Rust 完成 TTS 请求准备，启动命令返回后会再次按请求 ID 取消，避免遗留等待 ACK 的旧请求。无标点文本达到 180 字符或等待 900 ms 后分段，待播文本上限为 12,000 字符；超过上限停止当前回复并提示用户。当前改善了请求之间的空等，仍有每句建连成本，后续评估同一请求中的文本增量 streaming_call。

采集模块按音频样本数计时，在音频处理器完成一句后立即暂停收集；每次门控更新 epoch，丢弃已经排队的旧事件。固定缓冲保留 400 ms 前滚，100 ms 起音确认、120 ms 最小有声时长、850 ms 静音封段及 20 秒单段上限；界面动画不再负责 VAD。初始化、ASR 和 TTS 等待已增加阶段超时，细节及 PipeWire AEC 候选配置见 [VOICE_AUDIO_PIPELINE.md](VOICE_AUDIO_PIPELINE.md)。

若 Qwen 模型另有受支持的实时音频 API，可替换 ASR adapter。下一步优先验证 PipeWire 播放参考与可靠 AEC、建立专用 voice_session_id/turn_id，再优化 ASR 传输及测量真实延迟。

## 4. 逻辑架构与职责

| 层 | 职责 |
|---|---|
| React / Tauri WebView | AudioWorklet 捕获 PCM、执行按样本计时的 RMS endpointing；兼容环境使用相同 VAD 的 ScriptProcessor。固定 400 ms 前滚和 20 秒缓冲，封段后转为单声道 16 kHz WAV；回复期间以 epoch 门控暂停收集。普通 Agent 增量按 run ID 过滤并有界分句；Web Audio 串行调度 PCM16/22050 Hz 块，缓冲阈值约 800 ms，调度后 ACK。手动打断立即停止本地节点并联动取消 TTS/Agent。动态体在播报时读取播放 AnalyserNode。 |
| Rust Core | 校验文本/会话并传递带 request_id、sequence 的 TTS 块与 ACK；API Key、voice_id、Workspace 只从 Rust 配置/密钥边界读取，原始音频不落库。TTS 传输仍使用本地认证 JSON WebSocket 和 base64 块，ASR 也仍是 base64 WAV 请求。 |
| Python Voice Session（Agent sidecar 内） | 处理一次性 ASR 与 CosyVoice 合成请求；callback 线程桥接到最多 3 块的 asyncio 队列，sidecar 每次仅发送一个未确认 PCM 块，并等待 WebView ACK 后继续。取消消息会取消 TTS task 并调用 SDK streaming_cancel。 |
| 阿里云 | Qwen ASR 与 CosyVoice TTS；HTTPS/WSS，由受控运行时使用密钥。 |

Linux 桌面端使用 WebKitGTK。主窗口显式处理 WebKitGTK `permission-request`：通话面板发起的纯音频采集请求自动放行，视频请求拒绝；输入设备列表由 `enumerateDevices()` 提供，权限通过后可读取系统设备名称。若仍打不开麦克风，检查 PipeWire/PulseAudio 服务和系统输入静音状态。

声卡 I/O 在 renderer/WebView；turn 编排及云服务适配放 Python sidecar；Rust 持有会话与凭据边界、协议协调和落库。桌面端不新增独立语音守护进程。Android 若 WebView 采集/AEC 表现不足，再评估原生音频插件，但保持相同 Voice Session 协议。当前 TTS PCM 跨层使用有界 base64 JSON 块和逐块 ACK；ASR 音频仍是 base64 WAV 请求。后续可将 ASR 改为二进制专用通道。Agent Protocol 保持现有 127.0.0.1 WebSocket 主从关系。

## 5. 语音状态机与抢话

### 会话状态

当前 UI 状态为 idle → connecting → listening → recognizing → thinking ↔ speaking → listening；打断经 stopping 后恢复 listening。关闭面板即结束通话。回复期间保留麦克风 track，但 PCM 收集与本地 VAD 暂停。识别／Agent／TTS 错误在面板内提示，收尾后恢复聆听；麦克风连接错误允许重新开启。

### Utterance 生命周期

1. 开始会话时请求麦克风权限并初始化音频设备，持续显示麦克风开启状态。
2. 音频帧送本地 VAD，同时保留 300–500 ms 前滚缓冲以免切掉首辅音。VAD 起音后收集 utterance，静音达到可调 endpointing 窗口后封段。
3. utterance 完成后创建 ASR 请求，提交 WAV/PCM。每段有大小/时长上限；超长句按安全静音点切分或提示，不能无限缓存。
4. 只有 final transcript 创建 Agent turn 并作为用户消息写入 session。空识别、低置信或服务错误不触发模型回复。
5. Agent 正文增量按短句、标点、最大等待时间与最大字符数综合分段，禁止逐 token 请求 TTS。
6. 当前手动点击打断或按 Esc，立即停止播放器、清空队列、过滤旧 run，并取消 Agent run 和 TTS task。未来稳定 AEC 后才在播放期间恢复 VAD 自动插话。
7. 打断收尾后恢复采集；新 utterance endpointing 后识别并创建新 run。旧 run 的迟到事件不能恢复播报或重置新运行状态。专用 turn ID 及云端 ASR 请求取消尚待实现。
8. 用户结束、切换 session、权限撤销或连接失败时，取消任务、关闭连接、释放 AudioContext/track。

当前通过手动打断提供继续说话的入口；“嗯/对”等 backchannel 智能忽略暂不纳入 MVP。自动插话必须先验证扬声器播放参考、AEC 和双讲检测；VAD 或提高音量阈值不能替代回声消除。

## 6. 协议草案

以下为待落地的协议草案。控制事件使用现有 JSON envelope 风格；目标音频通道采用二进制 WebSocket frame 或 Tauri 专用通道，替换当前 base64 JSON。voice_session_id 与 turn_id 由 Rust 创建/校验。协议以兼容方式扩展 `src-tauri/src/agent/protocol.rs`，并同步更新 Python 镜像 `agent/app/protocol.py`。

| 方向 | 事件 | 用途 |
|---|---|---|
| UI → Rust | voice_start | 开始语音并绑定 agent、session、配置引用 |
| Rust → UI | voice_started / voice_state | 确认启动和状态变化 |
| UI → Rust → Python | voice_audio_frame | 序号、采样率、声道、时间戳及 PCM 二进制帧 |
| Python → Rust → UI | voice_transcript_partial | 临时识别展示；首选 API 暂无证明，协议预留 |
| Python → Rust → UI | voice_transcript_final | 最终识别；Rust 落用户消息 |
| Python → Rust → UI | voice_assistant_delta | 当前 turn 可播报文本；避免与普通 assistant delta 重复落库 |
| Python → Rust → UI | voice_audio_chunk | 规划中的编码、序号、turn_id 与音频二进制；当前 TTS PCM 块经 base64 JSON 传输 |
| UI → Rust → Python | voice_interrupt | 本地 VAD/用户打断；立即清播放队列 |
| Rust → Python/UI | voice_synthesize_stream_request / voice_synthesize_audio_chunk / voice_synthesize_ack / voice_synthesize_cancel | 当前已实现的 TTS 请求、PCM 块、播放 ACK 和请求取消；每块含 request_id 与 sequence |
| Rust → Python/UI | voice_cancel_turn / voice_turn_cancelled | 规划中的 Agent/TTS 话轮级取消并确认 turn 失效；当前只有单个 TTS 请求取消 |
| UI → Rust → Python | voice_stop | 结束会话并清理资源 |
| Python → Rust → UI | voice_error | 归一化错误、阶段与可恢复性 |

帧需有最大长度、背压和有界队列；队列满时丢弃过期帧或取消 turn，禁止无界占内存。接收端先验证 session/turn 再播放。最终 DTO 由 Rust 结构体定义。

当前额外实现 `agent://run_started`（Rust → WebView Tauri 事件）：在发送 sidecar `run_request` 前发布 session_id/run_id。语音面板发送前就绑定自己的 run ID，聊天 Store 从启动事件绑定运行；`send_message` 支持可选 `requestedRunId` 并返回实际 run ID，`cancel_run` 支持可选 `expectedRunId`，避免迟到取消操作误伤下一轮。它们不改变 Python Agent Protocol；专用语音会话/话轮 envelope 仍为规划内容。

## 7. Agent 与消息落库

- ASR partial 仅临时展示，不写数据库、不同步。
- ASR final 对应一条 user 消息；assistant 文本只保存一份。TTS 是文本的音频呈现，不创建另一条消息。
- 被插话取消的 assistant 文本遵循现有 run 取消和消息持久化语义；协议开发前检查 schema 是否能标记 interrupted。不得把尚未播放的 TTS 内容伪装成用户已听到的回复。
- 键盘消息和语音消息按创建序号进入同一 session；切换 session 前先结束当前语音会话。
- 原始音频不保存；未来若增加录音，作为独立加密 artifact 和用户选项设计。

## 8. 配置与凭据

首版配置字段建议：asr_provider=aliyun_dashscope、asr_model=qwen-audio-3.1-asr-flash、tts_provider=aliyun_dashscope、tts_model=cosyvoice-v3.5-flash、region/workspace_endpoint、可选 voice、language=zh-CN、音频格式、endpointing_ms、input_limit、tts_sentence_buffer。

不要错误复用 speech_model。若纳入 Model Routing，应新增明确 ASR/TTS role，ASR 不是“文本输出模型”能力。API Key 存 Rust SecretStore，sidecar 仅在会话期间按最小必要范围使用；禁止进入 React state/localStorage、普通设置 JSON、日志、消息和同步 payload。示例 endpoint 含 Workspace ID，实际地域和 Workspace 应可配置，且与密钥分开保存。

设置界面已增加“语音”页：ASR/TTS 模型名、Workspace ID、CosyVoice voice_id 保存在设备本地设置；DashScope API Key 存入 Rust SecretStore，兼容已有 DashScope 服务商 Key 和 `DASHSCOPE_API_KEY` 环境变量。音色描述经 sidecar 调用 Voice Design API，返回的预览 WAV 仅在界面内播放，不写入会话或持久化；用户确认后保存 voice_id。

## 9. 模块建议

建议前端新增 VoiceCallPanel、audioCapture、audioPlayback、voice protocol 模块；Rust 新增 voice 子模块，负责 commands/config/session/protocol；Python 新增 agent/app/voice/session.py、audio.py、providers/base.py、dashscope_asr.py、dashscope_tts.py。

Provider 端口应允许替换：transcribe(audio, config) → transcript；synthesize_stream(text, config) → AsyncIterator[AudioChunk]。若当前 SDK 只能整段返回，则公开 synthesize() 并声明 non-streaming，不伪造流式能力。VAD 也使用 provider-neutral 接口；先用 AudioWorklet 原型，按噪声/回声测试决定 WebRTC VAD 或平台能力，不把判断逻辑放在 React 组件中。

## 10. 对话界面与动态视觉设计

### 10.1 体验定位

语音通话使用独立的沉浸式工作区，不把语音流程塞进普通聊天气泡。用户点击聊天输入区的麦克风按钮后进入 `VoiceCallPanel`；面板占据当前工作区，保留窗口标题栏和返回/结束入口。通话舞台使用深色 `ink` 背景，压低普通应用 chrome 的视觉权重；动态体使用 Agnes 的 clay/coral 主色并辅以浅青色高光。桌面端以大面积留白和中心动态体表现“正在与 Agnes 对话”，移动端保持同一层级但缩小动态体和控制区间距。

视觉参考 SpeakRail 与 GPT Live 的连续呼吸效果，但不复制具体品牌图形。中心视觉是一个由实时音量和状态驱动的 `VoicePresence` Canvas：

- `listening`：低速呼吸，响应麦克风 RMS；使用偏暖的 clay/coral 和浅青色高光，表示 Agnes 正在听。
- `recognizing`：保持收缩呼吸，外围出现短促的环形扫描，表示正在等待 ASR final。
- `thinking`：动态体缓慢偏移和旋转，亮度较低，避免让用户误以为已经开始说话。
- `speaking`：根据 TTS 播放 RMS 和音频进度产生更明显的形变和光晕，首个音频块到达即切换，不等待整段完成。
- `stopping` / `interrupted`：快速收缩并在 180 ms 内停止动画与播放队列，然后回到 `listening`。
- `error`：暂停形变，使用单一警示色和可重试操作；错误信息放在状态文本区，不通过颜色单独传达。

动态体是当前任务的主要反馈，不作为装饰背景。实现优先使用 Canvas 2D + `requestAnimationFrame`，从 `AnalyserNode` 读取输入/输出能量；不保存原始音频。需要 WebGL 时再替换渲染器，但保持相同的状态和音量输入接口。支持 `prefers-reduced-motion`：减少位移和光晕，只保留静态状态色与轻微缩放。

### 10.2 面板结构

1. 顶部：会话标题、当前 Agent 名称、连接状态和结束按钮。结束是明确的挂断操作，不能与返回聊天混淆。
2. 中央：`VoicePresence` 动态体及状态标签，例如“正在听”“正在思考”“Agnes 正在说”。状态标签与 `VoiceState` 一一对应。
3. 下方：最近一轮的临时转写和最终转写。临时文本使用较低对比度，final transcript 与 assistant 文本只保留最近几轮，完整内容继续进入普通会话历史。
4. 底部：独立的“打断回复”按钮、麦克风静音/取消静音、扬声器静音和设备选择。打断支持 Esc；麦克风按钮暂停采集。识别或回复期间禁用设备切换，避免重启采集打断当前状态。
5. 错误与权限：首次进入显示麦克风权限状态；设备断开、权限撤销、网络错误都在面板内提供恢复动作，不把密钥、Workspace 或 SDK 错误细节展示给用户。

### 10.3 音频与状态同步

- UI 以 Rust/Python 的 `voice_state`、`voice_transcript_*`、`voice_audio_chunk` 事件作为状态源，不能仅根据本地按钮推断 Agent 阶段。
- `voice_audio_chunk` 到达后立即进入有界播放队列，同时更新 `speaking` 的音频能量；插话事件先清空队列，再等待新 turn 的音频。
- 当 turn_id 变化时，旧字幕和旧音频都停止更新；动态体只响应当前 turn，避免取消后的迟到事件造成闪回。
- 面板关闭、切换会话、窗口失焦策略触发或设备错误时，释放 `MediaStreamTrack`、`AudioContext` 和播放节点。

### 10.4 响应式与可访问性

- 桌面宽屏使用约 320–440 px 的动态体尺寸；窄屏以视口宽度的 52–68% 限制尺寸，控制区始终在安全区域内。
- 不使用大段说明文字解释功能；通过状态标签、图标 tooltip、字幕和可操作错误反馈完成引导。
- 所有状态不能只依赖颜色；必须同时有文字或图标变化。动态体和状态标签使用 `aria-live="polite"`，错误使用 `role="alert"`。
- 键盘支持开始/结束、静音和打断；动画和音量反馈不影响文本对话和系统级辅助功能。

这部分 UI 对应 `src/components/VoiceCallPanel.tsx`。当前已接入 ASR、Agent run 与 TTS 播放；聆听阶段显示采集模块送来的输入音量，播报阶段读取播放音量并驱动形变，识别／思考阶段使用独立状态和等待动效。支持 reduced-motion，打断按钮有文字、焦点状态和 Esc 键入口；窄屏控制区可换行。VAD 已独立为公共模块，在支持环境中随 AudioWorklet 运行。

## 11. 分阶段计划

### Phase 0：接口与延迟 Spike

- 运行仓库内官方示例，确认 endpoint、音频格式、真实响应结构、错误映射、依赖版本和产品开通状态。
- 检查 ASR 单段长度、耗时与本地音频安全上传方式；避免将私人录音放在公开 URL。
- 查 DashScope SDK/API，确认 CosyVoice Flash 的流式消费方法、回调线程、取消方式、首音频时间、输出格式和完整耗时。
- 确认目标 Workspace、地域、凭据方式、费用和限流。
- 用至少 20 条普通话样本（静音、短句、长句、噪声）测 VAD endpoint、ASR 完成、Agent 首文本、TTS 首字节/首音频及总延迟。
- 决策门：若只能整段 TTS 且首个可听音频 p50 超过体验目标，先验证流式 API 或实时 ASR，不投入 UI 全量接线。

### Phase 1：桌面端基本往返

- [x] 增加通话入口、权限提示、麦克风预览、通话状态舞台与结束控制。
- [x] 接通本地采集、本地 endpointing、整段 ASR 和识别文本进入现有 Agent。
- [x] 完成 CosyVoice 分句合成、WebView 播放队列和手动打断静音。
- [x] 完成 CosyVoice PCM callback 音频块的逐块回传、确认和 Web Audio 播放。
- [ ] 将当前 JSON base64 一次性 ASR 请求升级为有界二进制音频通道，减少复制和编码开销。
- 首版可整段 TTS；若 SDK 支持真正流式消费则启用。
- 转写和 assistant 文本复用现有消息仓储，不保存音频。

### Phase 2：低延迟与可靠抢话

- [x] 有界句子缓冲、无标点超时分段、串行 PCM 调度与下一句合成重叠。
- [x] 手动打断本地播放并取消 Agent/TTS；迟到事件按 run ID 过滤。
- [x] 识别／思考／播报状态、播放音量动效、通话关闭后的待发送文本撤销。
- [ ] 在 Arch Linux 扬声器与耳机环境实测首音频、句间停顿、恢复收听和快速打断；当前构建通过不代表设备验收通过。
- [x] AudioWorklet 采集与按样本计时的 VAD；兼容回退、采集 epoch 和固定缓冲。
- [x] 麦克风初始化、ASR UI 等待与 TTS 首音频/后续事件超时收尾，Provider 侧等待上限。
- [ ] 稳定 AEC 与播放参考：本机已具备 PipeWire WebRTC AEC 组件，monitor-mode 配置模板待真实路由、回灌与双讲验收。
- [ ] 自动语音插话及双讲检测、voice_session_id/turn_id 协议与 ASR 任务取消。
- [ ] 同一 CosyVoice 请求中的文本增量输入，进一步减少每句建连开销。
- [ ] Agent 阶段超时、弱网重连、限流与恢复策略；重试不能重复创建 user 消息或重复播放已消费音频。

### Phase 3：Android 与发布化

- 实测 Android 麦克风权限、AudioFocus、蓝牙、前后台生命周期、AEC 与 WebView 音频行为。
- 若需原生采集/播放，复用 Voice Session 协议与状态机。
- 完成并发上限、成本展示、隐私说明、密钥生命周期与发布态验收。

## 12. 验收目标

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

## 13. 风险与待确认事项

| 事项 | 当前处理 | 决策时点 |
|---|---|---|
| ASR 是否支持该模型实时输入 | 示例只证明 HTTP 整段请求，按非流式设计 | Phase 0 查正式 API 与 Workspace 权限 |
| CosyVoice Flash 文本增量接入 | 每个完成的 Agent 句子独立启动一个 PCM 流；当前 Agent `assistant_delta` 未直接驱动同一 TTS 请求的 `streaming_call()` | Phase 2 评估文本缓冲和双向 `streaming_call()` |
| ASR 音频提交方式 | 示例使用 input_audio.data URL | 确认受控上传/本地直传，避免公开 URL |
| 格式、最大长度、地域、价格、配额 | 不写死假设 | 实测和控制台确认后固化 |
| 首发音色 | 已使用 Voice Design 创建 Flash 专属音色，设置页支持设计、试听和保存 | 音色、模型、API Key 与 Workspace 必须一致 |
| 中断消息存储语义 | 遵循现有 run/message 能力 | 协议实现前检查 schema 与取消路径 |
| Android 交付范围 | Phase 3 | 桌面 MVP 验收后决定 |

## 14. 阶段完成定义

Phase 0 完成后必须有真实账户上的接口记录和延迟表；Phase 1 完成后用户能在桌面端与当前 Agent 多轮语音对话；Phase 2 完成后插话可即时静音、旧 turn 完整取消且文本历史不串轮；Phase 3 完成后 Android 通过目标设备验收，密钥与音频数据边界也经发布态检查。每阶段结束时更新本文档状态与实测结论，再进入下一阶段。
