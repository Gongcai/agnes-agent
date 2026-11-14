import React, { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "@phosphor-icons/react";
import {
  ensureCodeLanguageLoaded,
  formatCodeLanguageLabel,
  isCodeLanguageLoaded,
  resolveShikiLanguage,
  stripTrailingFenceNewline,
  subscribeCodeLanguageLoaded,
  tokenizeCodeSync,
  type ThemedToken,
} from "../lib/codeHighlight";

function tokenInlineStyle(token: ThemedToken): React.CSSProperties | undefined {
  const fontStyle = token.fontStyle ?? 0;
  const hasItalic = (fontStyle & 1) !== 0;
  const hasBold = (fontStyle & 2) !== 0;
  const hasUnderline = (fontStyle & 4) !== 0;

  if (!token.color && !hasItalic && !hasBold && !hasUnderline) {
    return undefined;
  }

  return {
    ...(token.color ? { color: token.color } : {}),
    ...(hasItalic ? { fontStyle: "italic" } : {}),
    ...(hasBold ? { fontWeight: 600 } : {}),
    ...(hasUnderline ? { textDecoration: "underline" } : {}),
  };
}

function diffLineClassName(lineText: string): string | undefined {
  if (lineText.startsWith("+") && !lineText.startsWith("+++")) {
    return "agnes-code-line-add";
  }
  if (lineText.startsWith("-") && !lineText.startsWith("---")) {
    return "agnes-code-line-del";
  }
  return undefined;
}

/** Code block card with Shiki syntax highlighting, normalized language label, and copy action. */
const CodeBlock: React.FC<{
  lang: string;
  text: string;
  children?: React.ReactNode;
}> = ({ lang, text }) => {
  const [copied, setCopied] = useState(false);
  const [grammarTick, setGrammarTick] = useState(0);

  const resolvedLang = useMemo(() => resolveShikiLanguage(lang), [lang]);
  const displayLabel = useMemo(() => formatCodeLanguageLabel(lang), [lang]);
  const cleanText = useMemo(() => stripTrailingFenceNewline(text), [text]);

  useEffect(() => {
    if (!resolvedLang || isCodeLanguageLoaded(resolvedLang)) return;

    let active = true;
    const unsubscribe = subscribeCodeLanguageLoaded(() => {
      if (active && isCodeLanguageLoaded(resolvedLang)) {
        setGrammarTick((tick) => tick + 1);
      }
    });
    void ensureCodeLanguageLoaded(resolvedLang).then((loaded) => {
      if (active && loaded) {
        setGrammarTick((tick) => tick + 1);
      }
    });

    return () => {
      active = false;
      unsubscribe();
    };
  }, [resolvedLang]);

  const tokens = useMemo(
    () => tokenizeCodeSync(cleanText, lang),
    [cleanText, lang, grammarTick],
  );

  const copy = () => {
    navigator.clipboard?.writeText(cleanText).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => {
        /* Ignore clipboard permission errors silently */
      },
    );
  };

  const isDiff = resolvedLang === "diff";

  return (
    <div className="markdown-code-block agnes-code-card my-3.5 rounded-lg">
      <div className="agnes-code-header flex items-center justify-between rounded-t-[7px] px-3.5 py-1.5">
        <div className="flex items-center gap-2 min-w-0">
          <span className="agnes-code-lang-dot h-1.5 w-1.5 shrink-0 rounded-full" aria-hidden="true" />
          <span className="agnes-code-lang font-mono text-[0.6875rem] font-medium tracking-tight truncate">
            {displayLabel}
          </span>
        </div>
        <button
          type="button"
          onClick={copy}
          className="agnes-code-copy-btn flex items-center gap-1 rounded-md px-2 py-0.5 text-[0.6875rem] transition-colors"
          title="复制代码"
        >
          {copied ? (
            <Check className="h-3.5 w-3.5 text-[#8cc487]" />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
          <span>{copied ? "已复制" : "复制"}</span>
        </button>
      </div>
      <pre className="agnes-code-pre overflow-x-auto overflow-y-hidden px-4 py-3.5 font-mono">
        <code className="font-mono">
          {tokens
            ? tokens.map((line, lineIndex) => {
                const rawLine = isDiff ? line.map((t) => t.content).join("") : "";
                const lineCls = isDiff ? diffLineClassName(rawLine) : undefined;
                return (
                  <React.Fragment key={lineIndex}>
                    <span className={lineCls}>
                      {line.map((token, tokenIndex) => (
                        <span key={tokenIndex} style={tokenInlineStyle(token)}>
                          {token.content}
                        </span>
                      ))}
                    </span>
                    {lineIndex < tokens.length - 1 ? "\n" : null}
                  </React.Fragment>
                );
              })
            : cleanText}
        </code>
      </pre>
    </div>
  );
};

export default CodeBlock;
