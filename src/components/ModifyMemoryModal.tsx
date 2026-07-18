import React, { useEffect, useState } from "react";
import { X, Trash2, Plus, ChevronDown } from "lucide-react";
import { useAgentStore, Message } from "../store/useAgentStore";

interface EditPart {
  kind: string;
  content: string;
  tool_call_id?: string;
  metadata?: string;
}

interface Props {
  message: Message | null;
  onClose: () => void;
}

const COMMON_TAG_KINDS = ["text", "thought", "tool_call", "tool_result", "model_fallback"];

/// 修改记忆弹窗：按顺序编辑某条 AI 消息的片段。
/// text 段=纯文本框；其它 kind 段=可折叠「标签行」（标签名=kind，展开编辑 content）。
/// 泛型识别：kind==="text" → 文本框，否则 → 标签行，不特判 thought/tool_call。
export const ModifyMemoryModal: React.FC<Props> = ({ message, onClose }) => {
  const replaceMessageParts = useAgentStore((s) => s.replaceMessageParts);
  const [parts, setParts] = useState<EditPart[]>([]);
  const [newKind, setNewKind] = useState("thought");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (message) {
      setParts(
        message.parts.map((p) => ({
          kind: p.kind,
          content: p.content,
          tool_call_id: p.tool_call?.id,
          metadata: undefined,
        })),
      );
    }
  }, [message]);

  if (!message) return null;

  const update = (i: number, patch: Partial<EditPart>) =>
    setParts((arr) => arr.map((p, idx) => (idx === i ? { ...p, ...patch } : p)));

  const remove = (i: number) => setParts((arr) => arr.filter((_, idx) => idx !== i));

  const addPart = () => {
    setParts((arr) => [...arr, { kind: newKind, content: "" }]);
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await replaceMessageParts(message.id, parts);
      onClose();
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm">
      <div className="w-[640px] max-h-[80vh] border border-stone-200 bg-white rounded-2xl overflow-hidden shadow-2xl flex flex-col">
        <header className="px-5 py-3.5 border-b border-stone-200 bg-stone-50 flex justify-between items-center shrink-0">
          <div className="flex items-center gap-2">
            <span className="font-semibold text-stone-800 text-sm">修改记忆</span>
            <span className="text-[10px] text-stone-400">编辑这条 AI 回复的片段，影响后续上下文</span>
          </div>
          <button
            onClick={onClose}
            className="text-stone-400 hover:text-stone-800 rounded p-1 hover:bg-stone-100 transition-colors"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          {parts.length === 0 && (
            <p className="text-center text-xs text-stone-400 py-8">没有任何片段，可新增一个。</p>
          )}
          {parts.map((p, i) => (
            <div key={i} className="border border-stone-200 rounded-xl overflow-hidden">
              {p.kind === "text" ? (
                <div className="p-2.5">
                  <div className="flex items-center justify-between mb-1.5">
                    <span className="text-[10px] font-semibold uppercase tracking-wide text-stone-400">回复 · text</span>
                    <button
                      onClick={() => remove(i)}
                      className="p-1 rounded text-stone-400 hover:text-red-600 hover:bg-red-50"
                      title="删除该片段"
                    >
                      <Trash2 className="h-3 w-3" />
                    </button>
                  </div>
                  <textarea
                    value={p.content}
                    onChange={(e) => update(i, { content: e.target.value })}
                    className="w-full bg-stone-50 rounded-lg p-2.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y min-h-[80px]"
                  />
                </div>
              ) : (
                <details className="group">
                  <summary className="flex items-center gap-2 cursor-pointer px-3 py-2 text-xs font-semibold text-stone-600 select-none hover:bg-stone-50">
                    <ChevronDown className="h-3 w-3 group-open:rotate-180 transition-transform" />
                    <span className="px-1.5 py-0.5 rounded bg-violet-50 text-violet-600 border border-violet-200/60 text-[10px] font-mono">
                      {p.kind}
                    </span>
                    <span className="text-[10px] text-stone-400 truncate flex-1">
                      {p.content.slice(0, 60) || "(空)"}
                    </span>
                    <button
                      onClick={(e) => { e.preventDefault(); remove(i); }}
                      className="p-1 rounded text-stone-400 hover:text-red-600 hover:bg-red-50"
                      title="删除该片段"
                    >
                      <Trash2 className="h-3 w-3" />
                    </button>
                  </summary>
                  <div className="px-3 pb-2.5 pt-1 border-t border-stone-100">
                    <textarea
                      value={p.content}
                      onChange={(e) => update(i, { content: e.target.value })}
                      className="w-full bg-stone-50 rounded-lg p-2.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y min-h-[80px]"
                    />
                  </div>
                </details>
              )}
            </div>
          ))}

          {/* 新增片段 */}
          <div className="flex items-center gap-2 pt-2 border-t border-stone-100">
            <select
              value={newKind}
              onChange={(e) => setNewKind(e.target.value)}
              className="bg-stone-50 p-1.5 rounded-lg border border-stone-200/60 text-[11px] font-mono focus:outline-none focus:ring-1 focus:ring-stone-300"
            >
              {COMMON_TAG_KINDS.map((k) => (
                <option key={k} value={k}>{k}</option>
              ))}
              {!COMMON_TAG_KINDS.includes(newKind) && <option value={newKind}>{newKind}</option>}
            </select>
            <input
              value={newKind}
              onChange={(e) => setNewKind(e.target.value)}
              placeholder="或输入自定义标签"
              className="flex-1 bg-stone-50 p-1.5 rounded-lg border border-stone-200/60 text-[11px] font-mono focus:outline-none focus:ring-1 focus:ring-stone-300"
            />
            <button
              onClick={addPart}
              className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[11px] font-semibold bg-stone-100 text-stone-600 hover:bg-stone-200"
            >
              <Plus className="h-3 w-3" />
              新增片段
            </button>
          </div>
        </div>

        <footer className="px-5 py-3 border-t border-stone-200 bg-stone-50 flex justify-end gap-2 shrink-0">
          <button
            onClick={onClose}
            className="px-4 py-1.5 rounded-lg text-xs font-semibold text-stone-500 hover:bg-stone-200/50"
          >
            取消
          </button>
          <button
            onClick={handleSave}
            disabled={saving}
            className="px-4 py-1.5 rounded-lg bg-[#8CA38A] text-white text-xs font-semibold hover:bg-[#7A917A] disabled:opacity-50"
          >
            {saving ? "保存中..." : "保存"}
          </button>
        </footer>
      </div>
    </div>
  );
};
