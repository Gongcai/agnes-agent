# 全双工语音 AI 交流

## 结论

可以实现。这里的“电话”指电话式的连续语音体验，不接入真实电话号码。当前项目已有三个可复用基础：模型 Provider 配置、`speech_model` 分工和 Python sidecar 的 WebSocket 流式 Agent。现有 `run_request -> assistant_delta -> run_finished` 是“轮次式文本聊天”协议，不能直接承载连续语音；语音能力应增加独立的实时语音会话层。

全双工的定义是：用户说话时 AI 可以继续播放已经生成的语音；用户插话后，系统检测到人声，立即停止尚未播放的 AI 音频并重新生成。单纯把录音文件送到 ASR、等待完整回复再 TTS，属于半双工，通话体验会明显变差。

## 推荐的第一版架构

```text
桌面/安卓麦克风与扬声器
    │  双向 PCM/Opus 音频流（16 kHz 或 24 kHz）
    ▼
Voice Session（sidecar 或独立本地服务）
    ├─ jitter buffer + codec 转换
    ├─ VAD / endpointing / barge-in
    ├─ Streaming ASR ── partial/final transcript ──┐
    │                                               ▼
    │                                      Agnes Agent Session
    │                                      memory + tools + LLM
    │                                               │ text delta
    └─ Streaming TTS ◄──────────────────────────────┘
             │ 立即返回音频帧，并接受 cancel
             ▼
       桌面/安卓音频播放
```

Voice Session 不应把每个音频 chunk 作为普通聊天消息写入 SQLite。它只维护短期语音状态；在一句话结束或会话结束时，再把用户转写、AI 回复和会话摘要写入现有 session/message/summary 链路。Tauri 端只负责采集、播放、设备权限和状态展示，音频编排放在 Python sidecar 或独立本地服务中。

建议先做一个 `voice_session` 抽象，接口至少包括：

- `start(call_id, agent_id, session_id, voice_config)`
- `push_audio(direction, seq, timestamp, bytes)`
- `push_transcript(partial|final, text, confidence)`
- `push_text_delta(text)`
- `push_audio_delta(codec, bytes)`
- `interrupt(reason)`、`stop(reason)`

现有 Agent sidecar 可增加专用的 `VOICE_TURN_START`、`VOICE_TRANSCRIPT_FINAL`、`VOICE_CANCEL`、`VOICE_AUDIO_DELTA` 事件；不要复用 `RUN_FINISHED` 作为语音边界。工具调用继续走当前 Rust 审批和审计链路，但高延迟或需要确认的工具应先发送简短语音占位语句，避免语音会话静默。

## ASR/TTS 选型

### 建议的默认组合

| 场景 | ASR | TTS | 选择理由 |
|---|---|---|---|
| 中国大陆普通话优先 | 阿里云智能语音交互实时识别（Paraformer 系列） | 阿里云百炼/智能语音的 CosyVoice 系列 | 中文口音、流式接口和国内网络更合适；同厂商接入与计费较简单 |
| 中国大陆低延迟备选 | 火山引擎豆包语音识别 | 火山引擎豆包语音合成 | 中文表现和实时流式产品成熟，适合做第二 Provider |
| 海外电话优先 | Deepgram Streaming ASR（Nova 系列） | Cartesia Sonic 或 ElevenLabs Flash 系列 | 端到端延迟低，电话音频和英语识别表现好 |
| 已采用 OpenAI 实时语音 | OpenAI Realtime 内置音频理解 | OpenAI Realtime 内置音频输出 | 可以省去 ASR/TTS 拼接和部分打断逻辑；需单独适配实时 API，不能假定 LiteLLM 文本接口可直接替代 |
| 企业合规/多语言 | Azure Speech 或 Google Cloud Speech/TTS | 同左 | 区域、合规、语言覆盖和企业采购更稳定 |

模型名称和可用区域会随服务商账号、地区及版本变化，Provider 配置应保存“服务商 + 模型 ID + API Base + 语音/语言参数”，而不是把模型名写死在代码里。`speech_model` 目前代表文本输出模型，不能直接当作流式 ASR 配置；应新增 `asr_provider/asr_model` 和 `tts_provider/tts_model` 两个独立配置。

## 我建议的落地顺序

1. **桌面/安卓麦克风闭环**：使用 WebSocket 或 WebRTC 传 PCM/Opus，接入一个流式 ASR 和一个流式 TTS，验证 VAD、partial transcript、音频队列和打断。
2. **复用现有 Agent**：把 final transcript 注入现有 session；Agent 文本 token 到达即送 TTS，TTS 音频按帧回传。每个语音轮次要有 `turn_id`，旧轮次的 ASR/TTS/LLM 结果全部可取消。
3. **生产化**：增加麦克风权限、耳机/扬声器选择、断线重连、Provider fallback、用量统计和会话结束摘要。录音默认关闭，转写文本遵循现有同步和 E2EE 边界。

## 延迟和打断门槛

第一版目标是：

- ASR partial 在 300 ms 内出现；一句话结束后 500 ms 内得到 final transcript。
- Agent 首个可播报文本在 800 ms–1.5 s 内到达（不含长工具调用）。
- TTS 首帧在 300 ms–600 ms 内到达，按 20–40 ms 音频帧播放。
- 检测到用户插话后 100 ms 内停止发送新的 AI 音频，并丢弃播放队列中未播放的数据。

必须做服务端/网关级 cancel：停止 TTS 生成、取消当前 Agent 语音轮次，并给 LLM 请求设置超时。只在 UI 层停止播放器会产生“AI 仍在后台说话、下一轮混入旧文本”的竞态。

## 与现有项目的改动边界

- 新增 `VoiceProvider` 端口；不要把二进制音频帧塞入 LiteLLM 的普通文本适配器。
- Rust 负责凭据、通话权限、session 绑定、审批和审计；API Key 继续放 OS Keyring/Android Keystore。
- Python sidecar 或独立本地 Voice Session 负责 Agent 语音轮次和文本流；它负责音频传输、VAD、codec、抖动缓冲和 TTS 播放队列。
- 新增 `audio` 输入能力标签，并把 ASR/TTS 配置从当前“语音理解模型”单字段拆开。
- 语音消息落库时保存 transcript 和 assistant 文本，音频只在用户显式开启录音时作为加密 artifact 保存。

因此，最稳妥的首发组合是“国内：阿里云 Paraformer + CosyVoice；海外：Deepgram + Cartesia/ElevenLabs；若优先追求最短链路则评估 OpenAI Realtime”。先做桌面/安卓实时闭环，再扩展其他音频入口，可以把设备采集、语音模型和 Agent 逻辑分开验证。
