import React, { useState } from "react";
import { Check, Copy } from "@phosphor-icons/react";

/// 代码块：深色背景 + 语言标签 + 复制按钮。
const CodeBlock: React.FC<{ lang: string; text: string; children: React.ReactNode }> = ({
  lang,
  text,
  children,
}) => {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard.writeText(text).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => {
        /* 静默 */
      }
    );
  };
  return (
    <div className="markdown-code-block my-3 rounded-lg border border-hairline bg-[#181715]">
      <div className="flex items-center justify-between rounded-t-[7px] border-b border-white/10 bg-[#1f1e1b] px-3.5 py-1.5 text-[0.6875rem] text-ink-muted">
        <div className="flex items-center gap-1.5">
          <span className="h-2 w-2 rounded-full bg-stone-600/60" />
          <span className="font-mono text-[0.625rem] text-stone-400">{lang || "text"}</span>
        </div>
        <button
          onClick={copy}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-stone-400 hover:bg-stone-800 hover:text-stone-200 transition-colors"
          title="复制代码"
        >
          {copied ? <Check className="h-3 w-3 text-emerald-400" /> : <Copy className="h-3 w-3" />}
          <span className="text-[0.625rem]">{copied ? "已复制" : "复制"}</span>
        </button>
      </div>
      <pre className="overflow-x-auto overflow-y-hidden p-3.5 text-[0.75rem] font-mono leading-relaxed text-[#e6dfd8]">
        <code className="font-mono">{children}</code>
      </pre>
    </div>
  );
};

export default CodeBlock;
