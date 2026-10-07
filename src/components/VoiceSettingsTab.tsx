import React, { useEffect, useState } from "react";
import { Copy, Play, SpeakerHigh, Trash } from "@phosphor-icons/react";
import { invoke } from "@tauri-apps/api/core";
import { VoiceAecSettings } from "./VoiceAecSettings";

interface VoiceSettings {
  api_key_configured: boolean;
  asr_model: string;
  tts_model: string;
  voice_id: string;
  workspace_id: string;
}

interface VoiceDesignResult {
  voice_id: string;
  preview_audio_base64: string | null;
  request_id: string | null;
}

const DEFAULT_SETTINGS: VoiceSettings = {
  api_key_configured: false,
  asr_model: "qwen-audio-3.1-asr-flash",
  tts_model: "cosyvoice-v3.5-flash",
  voice_id: "",
  workspace_id: "",
};

export const VoiceSettingsTab: React.FC = () => {
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  const [apiKey, setApiKey] = useState("");
  const [clearApiKey, setClearApiKey] = useState(false);
  const [voicePrompt, setVoicePrompt] = useState("");
  const [previewText, setPreviewText] = useState("你好，我是 Agnes。很高兴与你开始这段语音对话。");
  const [generatedVoice, setGeneratedVoice] = useState<VoiceDesignResult | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [isDesigning, setIsDesigning] = useState(false);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);

  const load = async () => {
    const result = await invoke<VoiceSettings>("get_voice_settings");
    setSettings(result);
  };

  useEffect(() => {
    void load().catch((error) => setNotice({ ok: false, text: String(error) }));
  }, []);

  const save = async () => {
    setIsSaving(true);
    setNotice(null);
    try {
      await invoke("set_voice_settings", {
        input: {
          api_key: apiKey.trim() || null,
          clear_api_key: clearApiKey,
          asr_model: settings.asr_model,
          tts_model: settings.tts_model,
          voice_id: settings.voice_id,
          workspace_id: settings.workspace_id.trim() || null,
        },
      });
      setApiKey("");
      setClearApiKey(false);
      await load();
      setNotice({ ok: true, text: "语音设置已保存" });
    } catch (error) {
      setNotice({ ok: false, text: String(error) });
    } finally {
      setIsSaving(false);
    }
  };

  const design = async () => {
    setIsDesigning(true);
    setNotice(null);
    setGeneratedVoice(null);
    try {
      await invoke("set_voice_settings", {
        input: {
          api_key: apiKey.trim() || null,
          clear_api_key: false,
          asr_model: settings.asr_model,
          tts_model: settings.tts_model,
          voice_id: settings.voice_id,
          workspace_id: settings.workspace_id.trim() || null,
        },
      });
      setApiKey("");
      await load();
      const result = await invoke<VoiceDesignResult>("design_voice", {
        input: { voice_prompt: voicePrompt, preview_text: previewText },
      });
      setGeneratedVoice(result);
      setSettings((current) => ({ ...current, voice_id: result.voice_id }));
      setNotice({ ok: true, text: "音色已生成，请试听后保存配置" });
    } catch (error) {
      setNotice({ ok: false, text: String(error) });
    } finally {
      setIsDesigning(false);
    }
  };

  const copyVoiceId = async (voiceId: string) => {
    await navigator.clipboard.writeText(voiceId);
    setNotice({ ok: true, text: "音色 ID 已复制" });
  };

  return (
    <div className="mx-auto max-w-2xl space-y-6">
      <div className="flex items-start justify-between gap-4 border-b border-stone-200 pb-3">
        <div>
          <h3 className="text-sm font-semibold text-stone-850">语音模型</h3>
          <p className="mt-1 text-[11px] text-stone-500">配置 DashScope 语音识别、合成与 Agnes 音色。</p>
        </div>
        <button type="button" onClick={() => void save()} disabled={isSaving} className="shrink-0 rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-indigo-700 disabled:opacity-50">
          {isSaving ? "保存中…" : "保存设置"}
        </button>
      </div>

      {notice && <div role={notice.ok ? "status" : "alert"} className={`border px-3 py-2 text-[11px] ${notice.ok ? "border-emerald-200 bg-emerald-50 text-emerald-700" : "border-rose-200 bg-rose-50 text-rose-700"}`}>{notice.text}</div>}

      <section className="space-y-3 border-b border-stone-200 pb-5">
        <h4 className="text-xs font-semibold text-stone-700">DashScope 凭据与模型</h4>
        <label className="block space-y-1">
          <span className="text-[10px] font-semibold text-stone-600">API Key {settings.api_key_configured && <span className="font-normal text-emerald-700">· 已配置</span>}</span>
          <input type="password" autoComplete="new-password" value={apiKey} onChange={(event) => { setApiKey(event.target.value); setClearApiKey(false); }} placeholder={settings.api_key_configured ? "输入新 Key 以替换现有密钥" : "sk-…"} className="w-full rounded-md border border-stone-200 px-3 py-2 text-xs outline-none focus:border-indigo-300 focus:ring-1 focus:ring-indigo-200" />
        </label>
        {settings.api_key_configured && <button type="button" onClick={() => { setClearApiKey((value) => !value); setApiKey(""); }} className={`inline-flex items-center gap-1.5 text-[10px] ${clearApiKey ? "text-rose-700" : "text-stone-500 hover:text-rose-700"}`}>
          <Trash size={13} />{clearApiKey ? "将删除已保存的 API Key" : "删除已保存的 API Key"}
        </button>}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <label className="block space-y-1">
            <span className="text-[10px] font-semibold text-stone-600">ASR 模型</span>
            <input value={settings.asr_model} onChange={(event) => setSettings((current) => ({ ...current, asr_model: event.target.value }))} className="w-full rounded-md border border-stone-200 px-3 py-2 text-xs font-mono outline-none focus:border-indigo-300" />
          </label>
          <label className="block space-y-1">
            <span className="text-[10px] font-semibold text-stone-600">TTS 模型</span>
            <input value={settings.tts_model} onChange={(event) => setSettings((current) => ({ ...current, tts_model: event.target.value }))} className="w-full rounded-md border border-stone-200 px-3 py-2 text-xs font-mono outline-none focus:border-indigo-300" />
          </label>
        </div>
        <label className="block space-y-1">
          <span className="text-[10px] font-semibold text-stone-600">Workspace ID（可选）</span>
          <input value={settings.workspace_id} onChange={(event) => setSettings((current) => ({ ...current, workspace_id: event.target.value }))} placeholder="例如 llm-srukcv1xajun2e5g" className="w-full rounded-md border border-stone-200 px-3 py-2 text-xs font-mono outline-none focus:border-indigo-300" />
          <p className="text-[10px] text-stone-400">只填写百炼控制台的 Workspace ID，不要填写域名、wss:// 或完整 URL。</p>
        </label>
        <label className="block space-y-1">
          <span className="text-[10px] font-semibold text-stone-600">当前音色 ID</span>
          <div className="flex gap-2">
            <input value={settings.voice_id} onChange={(event) => setSettings((current) => ({ ...current, voice_id: event.target.value }))} placeholder="cosyvoice-v3.5-flash-vd-…" className="min-w-0 flex-1 rounded-md border border-stone-200 px-3 py-2 text-xs font-mono outline-none focus:border-indigo-300" />
            {settings.voice_id && <button type="button" title="复制当前音色 ID" onClick={() => void copyVoiceId(settings.voice_id)} className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-stone-200 text-stone-500 hover:bg-stone-50"><Copy size={15} /></button>}
          </div>
        </label>
      </section>

      <VoiceAecSettings />

      <section className="space-y-3">
        <div className="flex items-center gap-2"><SpeakerHigh size={16} className="text-indigo-600" /><h4 className="text-xs font-semibold text-stone-700">设计新音色</h4></div>
        <label className="block space-y-1">
          <span className="text-[10px] font-semibold text-stone-600">音色描述 <span className="font-normal">（最多 500 字）</span></span>
          <textarea value={voicePrompt} onChange={(event) => setVoicePrompt(event.target.value.slice(0, 500))} rows={4} maxLength={500} placeholder="描述音色、语速、情绪和适用场景" className="w-full resize-y rounded-md border border-stone-200 px-3 py-2 text-xs leading-5 outline-none focus:border-indigo-300" />
          <div className="text-right text-[10px] text-stone-400">{voicePrompt.length}/500</div>
        </label>
        <label className="block space-y-1">
          <span className="text-[10px] font-semibold text-stone-600">试听文本</span>
          <input value={previewText} onChange={(event) => setPreviewText(event.target.value)} maxLength={300} className="w-full rounded-md border border-stone-200 px-3 py-2 text-xs outline-none focus:border-indigo-300" />
        </label>
        <button type="button" onClick={() => void design()} disabled={isDesigning || !voicePrompt.trim() || !settings.api_key_configured && !apiKey.trim()} className="inline-flex items-center gap-2 rounded-md border border-stone-300 px-3 py-2 text-xs font-semibold text-stone-700 hover:bg-stone-50 disabled:opacity-40">
          <Play size={14} weight="fill" />{isDesigning ? "正在生成音色…" : "生成并试听"}
        </button>
        {generatedVoice && <div className="space-y-3 border-y border-stone-200 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <code className="min-w-0 flex-1 break-all text-[11px] text-stone-700">{generatedVoice.voice_id}</code>
            <button type="button" onClick={() => void copyVoiceId(generatedVoice.voice_id)} title="复制音色 ID" className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-stone-500 hover:bg-stone-100"><Copy size={15} /></button>
          </div>
          {generatedVoice.preview_audio_base64 && <audio controls preload="metadata" className="h-9 w-full" src={`data:audio/wav;base64,${generatedVoice.preview_audio_base64}`} />}
          {!generatedVoice.preview_audio_base64 && <p className="text-[10px] text-amber-700">服务未返回试听音频，voice ID 已填入当前音色设置。</p>}
        </div>}
      </section>
    </div>
  );
};
