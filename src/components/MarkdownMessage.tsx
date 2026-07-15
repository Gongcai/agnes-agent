import React, { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import { Check, Copy } from "lucide-react";

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
        /* 剪贴板不可用时静默 */
      }
    );
  };
  return (
    <div className="my-3 rounded-lg overflow-hidden border border-stone-200 shadow-sm">
      <div className="bg-stone-100 px-3 py-1.5 flex justify-between items-center text-[10px] text-stone-500 border-b border-stone-200">
        <span className="font-mono">{lang || "text"}</span>
        <button
          onClick={copy}
          className="flex items-center gap-1 hover:text-stone-900 transition-colors"
        >
          {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <pre className="bg-zinc-900 p-3 overflow-x-auto text-zinc-100 text-[11px] font-mono leading-relaxed">
        <code className="font-mono">{children}</code>
      </pre>
    </div>
  );
};

interface MarkdownMessageProps {
  content: string;
  streaming?: boolean;
}

/// Markdown + LaTeX 渲染（中文优化）。用于助手消息文本。
const MarkdownMessageView: React.FC<MarkdownMessageProps> = ({ content, streaming = false }) => {
  // Parsing Markdown, GFM and KaTeX for every token can monopolize the renderer.
  // Keep the live path lightweight and apply rich formatting once the run finishes.
  if (streaming) {
    return (
      <div className="markdown-body whitespace-pre-wrap break-words">
        {content}
        <span className="ml-0.5 inline-block h-4 w-1 animate-pulse bg-stone-400 align-text-bottom" />
      </div>
    );
  }

  return (
    <div className="markdown-body">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeKatex]}
        components={{
          // 去除 react-markdown 默认的 <pre> 包裹，改由 code 自行渲染代码块
          pre: ({ children }) => <>{children}</>,
          code({ node, className, children, ...props }) {
            const text = String(children ?? "");
            const match = /language-(\w+)/.exec(className || "");
            const isBlock = !!match || text.includes("\n");
            if (isBlock) {
              return (
                <CodeBlock lang={match ? match[1] : ""} text={text}>
                  {children}
                </CodeBlock>
              );
            }
            return (
              <code
                className="px-1.5 py-0.5 rounded bg-stone-100 text-[0.85em] font-mono text-rose-600 border border-stone-200"
                {...props}
              >
                {children}
              </code>
            );
          },
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
};

export const MarkdownMessage = React.memo(MarkdownMessageView);
