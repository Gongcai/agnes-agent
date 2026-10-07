# Agnes 默认音色设计

## 设计目标

用于长期中文语音对话的原创助手音色：清晰、稳定、温和，不像新闻播音或广告配音；在解释复杂问题时保持可听性，在连续多轮对话中不疲劳、不夸张。

## CosyVoice Voice Design 参数

目标模型必须与合成模型一致：`cosyvoice-v3.5-flash`。声音描述限制在 CosyVoice 支持的 500 字符以内。

```text
一位三十岁左右的女性语音助手，中音偏低，音色温暖清澈，细腻而有质感；语速中等偏慢，吐字清晰自然，停顿短而从容；情绪稳定、耐心、亲切，带有轻微的微笑感，但不过分活泼或夸张；适合长时间的中文对话、解释复杂问题、提醒和陪伴式交互。避免播音腔、广告腔、尖锐高音和过度戏剧化。
```

预览文本：

```text
你好，我是 Agnes。今天我们一起把事情理清楚，我会用清晰、自然的语气陪你完成接下来的工作。
```

同一描述具有随机性。第一次试听后，如果音色不合适，可以使用不同 prefix 再创建一到两个候选；不要覆盖已在使用的 `voice_id`。

## 创建音色

脚本会调用 Voice Design API，目标模型为 `cosyvoice-v3.5-flash`，并把预览 WAV 写到当前目录：

```bash
cd agent
export DASHSCOPE_API_KEY='你的北京地域百炼 API Key'
export DASHSCOPE_WORKSPACE_ID='你的 Workspace ID'  # 使用 Workspace endpoint 时设置
uv run python -m app.voice.design_voice \
  --preview-output ../ProjectPlan/agnes-voice-preview.wav
```

如果不使用 Workspace endpoint，也可以显式设置：

```bash
export DASHSCOPE_VOICE_DESIGN_ENDPOINT='https://dashscope.aliyuncs.com/api/v1/services/audio/tts/customization'
```

脚本成功后会输出：

```text
target_model=cosyvoice-v3.5-flash
voice_id=<generated voice id>
export DASHSCOPE_TTS_VOICE=<generated voice id>
```

先播放 `agnes-voice-preview.wav` 确认音色，再将 `voice_id` 配到 TTS smoke 测试。不要把 API Key 写入文件或提交 Git；`voice_id` 本身不是密钥，但应和对应地域/Workspace 一起保存到本机语音设置。

## 合成配置

```bash
export DASHSCOPE_API_KEY='你的北京地域百炼 API Key'
export DASHSCOPE_TTS_VOICE='<generated voice id>'
export DASHSCOPE_TTS_WEBSOCKET_URL='wss://<WorkspaceId>.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference'
export DASHSCOPE_WORKSPACE_ID='<WorkspaceId>'

cd agent
uv run python -m app.voice.spike \
  --text '你好，我是 Agnes。现在开始语音对话。' \
  --output /tmp/agnes-voice-test.mp3
```

Voice Design 与语音合成必须使用同一个模型、地域和可访问的 Workspace。若返回 `InvalidParameter` 或 `418`，优先检查四项：`voice_id` 是否是刚为 Flash 创建的音色、TTS model 是否仍为 `cosyvoice-v3.5-flash`、API Key 是否为北京地域、WebSocket endpoint 是否属于同一 Workspace。

## 当前状态

- 音色描述：已确定。
- 创建请求：脚本已实现，等待用户环境中的 API Key/Workspace 执行。
- 预览试听：待执行。
- 固化 `voice_id`：试听通过后再写入应用语音配置，不写入仓库明文配置。
