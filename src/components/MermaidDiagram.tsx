import React, { useEffect, useId, useRef, useState } from "react";
import type { Mermaid } from "mermaid";
import CodeBlock from "./CodeBlock";

let mermaidPromise: Promise<Mermaid> | null = null;

function loadMermaid(): Promise<Mermaid> {
  if (!mermaidPromise) {
    mermaidPromise = import("mermaid").then((module) => module.default);
  }
  return mermaidPromise;
}

function isDarkTheme(): boolean {
  return document.documentElement.getAttribute("data-agnes-theme") === "dark";
}

interface MermaidDiagramProps {
  code: string;
}

/// Mermaid 图表渲染：解析失败时回退为普通代码块（流式输出中的半成品也会走到这里）。
const MermaidDiagram: React.FC<MermaidDiagramProps> = ({ code }) => {
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [showSource, setShowSource] = useState(false);
  const renderId = useId().replace(/[^a-zA-Z0-9]/g, "");
  const latestCodeRef = useRef(code);
  latestCodeRef.current = code;

  useEffect(() => {
    let cancelled = false;

    const render = async (dark: boolean) => {
      try {
        const mermaid = await loadMermaid();
        // mermaid.render 会把结果写入 DOM，流式/切换场景下可能被后续渲染覆盖，校验 code 仍是最新值。
        const currentCode = latestCodeRef.current;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: dark ? "dark" : "neutral",
        });
        await mermaid.parse(currentCode);
        const { svg: rendered } = await mermaid.render(`mermaid-${renderId}`, currentCode);
        if (cancelled || latestCodeRef.current !== currentCode) return;
        setSvg(rendered);
        setFailed(false);
      } catch {
        if (cancelled) return;
        setSvg(null);
        setFailed(true);
      }
    };

    void render(isDarkTheme());

    // 跟随应用明暗主题切换重新渲染。
    const observer = new MutationObserver(() => {
      if (!cancelled) void render(isDarkTheme());
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-agnes-theme"],
    });

    return () => {
      cancelled = true;
      observer.disconnect();
    };
  }, [code, renderId]);

  if (failed) {
    return <CodeBlock lang="mermaid" text={code}>{code}</CodeBlock>;
  }

  return (
    <div className="mermaid-diagram my-3 rounded-lg border border-hairline bg-canvas-surface-card">
      <div className="flex items-center justify-between rounded-t-[7px] border-b border-hairline px-3.5 py-1.5 text-[0.6875rem] text-ink-muted">
        <div className="flex items-center gap-1.5">
          <span className="h-2 w-2 rounded-full bg-stone-400/60" />
          <span className="font-mono text-[0.625rem]">mermaid</span>
        </div>
        <button
          onClick={() => setShowSource((value) => !value)}
          className="rounded px-1.5 py-0.5 hover:bg-stone-800/10 transition-colors"
          title={showSource ? "查看图表" : "查看源码"}
        >
          <span className="text-[0.625rem]">{showSource ? "图表" : "源码"}</span>
        </button>
      </div>
      {showSource ? (
        <pre className="overflow-x-auto p-3.5 text-[0.75rem] font-mono leading-relaxed text-ink-muted">
          <code className="font-mono">{code}</code>
        </pre>
      ) : (
        <div
          className="mermaid-svg flex justify-center overflow-x-auto p-3"
          // mermaid 输出为受控字符串（parse 通过 + securityLevel strict），非任意 HTML。
          dangerouslySetInnerHTML={{ __html: svg ?? "" }}
        />
      )}
    </div>
  );
};

export default MermaidDiagram;
