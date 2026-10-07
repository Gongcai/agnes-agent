import os
import time

import dashscope
from dashscope.audio.tts_v2 import (
    AudioFormat,
    SpeechSynthesizer,
    VoiceEnrollmentService,
)

dashscope.base_websocket_api_url = (
    "wss://llm-srukcv1xajun2e5g.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference"
)
dashscope.base_http_api_url = (
    "https://llm-srukcv1xajun2e5g.cn-beijing.maas.aliyuncs.com/api/v1"
)
dashscope.api_key = os.getenv("DASHSCOPE_API_KEY")
if not dashscope.api_key:
    raise ValueError("DASHSCOPE_API_KEY environment variable not set.")

TARGET_MODEL = "cosyvoice-v3.5-flash"
VOICE_PREFIX = "myvoice"
# Publicly accessible sample audio. Replace with your own audio URL as needed.
AUDIO_URL = "https://dashscope.oss-cn-beijing.aliyuncs.com/samples/audio/cosyvoice/cosyvoice-zeroshot-sample.wav"

print("--- Step 1: Creating voice enrollment ---")
service = VoiceEnrollmentService()
voice_id = service.create_voice(
    target_model=TARGET_MODEL,
    prefix=VOICE_PREFIX,
    url=AUDIO_URL,
)
print(f"Voice creation submitted. Request ID: {service.get_last_request_id()}")
print(f"Generated Voice ID: {voice_id}")

print("\n--- Step 2: Polling for voice status ---")
max_attempts = 30
poll_interval = 10
for attempt in range(max_attempts):
    voice_info = service.query_voice(voice_id=voice_id)
    status = voice_info.get("status")
    print(f"Attempt {attempt + 1}/{max_attempts}: Voice status is '{status}'")
    if status == "OK":
        break
    if status == "UNDEPLOYED":
        raise RuntimeError(f"Voice processing failed. Voice ID: {voice_id}")
    if attempt < max_attempts - 1:
        time.sleep(poll_interval)
else:
    raise TimeoutError(
        f"Voice was not ready within the polling limit. Voice ID: {voice_id}"
    )

print("\n--- Step 3: Synthesizing speech with the new voice ---")
synthesizer = SpeechSynthesizer(
    model=TARGET_MODEL,
    voice=voice_id,
    format=AudioFormat.MP3_22050HZ_MONO_256KBPS,
)
audio_data = synthesizer.call("How is the weather today?")
if not audio_data:
    raise RuntimeError("Speech synthesis returned no audio.")
print(f"Speech synthesis successful. Request ID: {synthesizer.get_last_request_id()}")
output_file = "my_custom_voice_output.mp3"
with open(output_file, "wb") as f:
    f.write(audio_data)
print(f"Audio saved to {output_file}")
