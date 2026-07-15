import React, { useEffect, useMemo, useRef, useState } from "react";
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

const MARKDOWN_RENDER_INTERVAL_MS = 400;

/// Split at safe blank-line boundaries so completed blocks retain their React cache.
function splitMarkdownBlocks(content: string): string[] {
  // Reference-style links and footnotes can resolve across blank lines, so keep
  // those uncommon documents as one semantic unit.
  if (/^\s*\[(?:\^)?[^\]]+\]:/m.test(content)) return content.trim() ? [content] : [];

  const lines = content.match(/[^\n]*\n|[^\n]+$/g) ?? [];
  const nextNonEmptyLines = new Array<string>(lines.length).fill("");
  let nextNonEmptyLine = "";
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    nextNonEmptyLines[index] = nextNonEmptyLine;
    if (lines[index].trim()) nextNonEmptyLine = lines[index];
  }
  const blocks: string[] = [];
  let current = "";
  let fence: { char: string; length: number } | null = null;
  let inDisplayMath = false;

  lines.forEach((line, index) => {
    current += line;
    const trimmed = line.trim();
    const fenceMatch = /^(`{3,}|~{3,})/.exec(trimmed);
    if (fenceMatch) {
      const marker = fenceMatch[1];
      if (!fence) {
        fence = { char: marker[0], length: marker.length };
      } else if (marker[0] === fence.char && marker.length >= fence.length) {
        fence = null;
      }
    } else if (!fence && (trimmed === "$$" || trimmed === "\\[" || trimmed === "\\]")) {
      inDisplayMath = !inDisplayMath;
    }

    if (trimmed || fence || inDisplayMath) return;

    const firstLine = current.trimStart().split("\n", 1)[0] ?? "";
    const nextRawLine = nextNonEmptyLines[index];
    const nextLine = nextRawLine.trimStart();
    const currentIsList = /^(?:[-+*]|\d+\.)\s/.test(firstLine);
    const nextContinuesList = /^(?:[-+*]|\d+\.)\s/.test(nextLine) || /^\s{2,}\S/.test(nextRawLine);
    const currentIsQuote = firstLine.startsWith(">");
    const nextContinuesQuote = nextLine.startsWith(">");
    if ((currentIsList && nextContinuesList) || (currentIsQuote && nextContinuesQuote)) return;

    if (current.trim()) blocks.push(current);
    current = "";
  });

  if (current.trim()) blocks.push(current);
  return blocks;
}

const MarkdownBlock = React.memo<{ content: string }>(({ content }) => (
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
));

/// Markdown + LaTeX 渲染（中文优化）。用于助手消息文本。
const MarkdownMessageView: React.FC<MarkdownMessageProps> = ({ content, streaming = false }) => {
  const [renderedContent, setRenderedContent] = useState(content);
  const latestContentRef = useRef(content);
  const renderTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    latestContentRef.current = content;

    if (!streaming) {
      if (renderTimerRef.current !== null) {
        clearTimeout(renderTimerRef.current);
        renderTimerRef.current = null;
      }
      if (renderedContent !== content) {
        React.startTransition(() => setRenderedContent(content));
      }
      return;
    }

    if (renderTimerRef.current === null && renderedContent !== content) {
      renderTimerRef.current = setTimeout(() => {
        renderTimerRef.current = null;
        React.startTransition(() => setRenderedContent(latestContentRef.current));
      }, MARKDOWN_RENDER_INTERVAL_MS);
    }
  }, [content, renderedContent, streaming]);

  useEffect(() => () => {
    if (renderTimerRef.current !== null) clearTimeout(renderTimerRef.current);
  }, []);

  const cachedContent = content.startsWith(renderedContent) ? renderedContent : "";
  const liveTail = content.slice(cachedContent.length);
  const blocks = useMemo(() => splitMarkdownBlocks(cachedContent), [cachedContent]);

  return (
    <div className="markdown-body">
      {blocks.map((block, index) => (
        <MarkdownBlock key={index} content={block} />
      ))}
      {liveTail && <span className="whitespace-pre-wrap break-words">{liveTail}</span>}
      {streaming && (
        <span className="ml-0.5 inline-block h-4 w-1 animate-pulse bg-stone-400 align-text-bottom" />
      )}
    </div>
  );
};

export const MarkdownMessage = React.memo(MarkdownMessageView);
