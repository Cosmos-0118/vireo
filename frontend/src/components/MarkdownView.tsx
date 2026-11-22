import React, { useEffect, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import type { TokenSpan } from '../syntax';
import type { LiteTheme } from '../themes';

const FencedCodeBlock: React.FC<{
  lang: string;
  code: string;
  theme: LiteTheme;
}> = ({ lang, code, theme }) => {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // ignore clipboard errors
    }
  };

  const normalizedLang = lang.trim().toLowerCase();
  const isDiff = normalizedLang === 'diff';
  // The Lezer grammars are heavy; load them on first code block instead of at app start.
  const [highlighted, setHighlighted] = useState<TokenSpan[][] | null>(null);
  useEffect(() => {
    if (!normalizedLang) {
      setHighlighted(null);
      return;
    }
    let cancelled = false;
    import('../syntax')
      .then((m) => {
        if (!cancelled) setHighlighted(m.highlightLinesByName(code, normalizedLang, theme));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [code, normalizedLang, theme]);
  const rawLines = code.split(/\r?\n/);

  return (
    <div className="my-2 rounded-md border border-[var(--border)] bg-[var(--bg-panel)] overflow-hidden">
      <div className="flex items-center justify-between px-2.5 py-1 border-b border-[var(--border)]/80 bg-[var(--bg-elevated)]/60">
        <span className="text-[10.5px] font-mono uppercase tracking-wider text-[var(--text-muted)]">
          {normalizedLang || 'code'}
        </span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-1 px-1.5 py-0.5 text-[10.5px] rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
          title="Copy code block"
        >
          {copied ? (
            <>
              <Check size={11} className="text-[var(--added-fg)]" />
              <span className="text-[var(--added-fg)]">Copied</span>
            </>
          ) : (
            <>
              <Copy size={11} />
              <span>Copy</span>
            </>
          )}
        </button>
      </div>
      <pre className="p-2.5 overflow-x-auto font-mono text-[12px] leading-[1.55] text-[var(--text-primary)] m-0">
        {highlighted ? (
          <code>
            {highlighted.map((lineSpans, idx) => (
              <div key={idx} className="min-h-[1.55em]">
                {lineSpans.length > 0
                  ? lineSpans.map((sp, sIdx) => (
                      <span
                        key={sIdx}
                        style={{
                          color: sp.color,
                          fontStyle: sp.italic ? 'italic' : undefined,
                          fontWeight: sp.bold ? 500 : undefined,
                          textDecoration: sp.underline ? 'underline' : sp.strike ? 'line-through' : undefined,
                        }}
                      >
                        {sp.text}
                      </span>
                    ))
                  : '\u00A0'}
              </div>
            ))}
          </code>
        ) : isDiff ? (
          <code>
            {rawLines.map((line, idx) => {
              const isAdd = line.startsWith('+') && !line.startsWith('+++');
              const isRem = line.startsWith('-') && !line.startsWith('---');
              const isHunk = line.startsWith('@@');
              return (
                <div
                  key={idx}
                  className="px-1 -mx-1"
                  style={{
                    background: isAdd
                      ? 'var(--added-bg)'
                      : isRem
                      ? 'var(--removed-bg)'
                      : undefined,
                    color: isAdd
                      ? 'var(--added-fg)'
                      : isRem
                      ? 'var(--removed-fg)'
                      : isHunk
                      ? 'var(--accent)'
                      : undefined,
                  }}
                >
                  {line || '\u00A0'}
                </div>
              );
            })}
          </code>
        ) : (
          <code>{code}</code>
        )}
      </pre>
    </div>
  );
};

function renderInlineMarkdown(text: string): React.ReactNode[] {
  // Tokenize inline code, bold, italic, and links
  const regex = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]]+\]\((https?:\/\/[^\s)]+)\))/g;
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(text.slice(lastIndex, match.index));
    }
    const token = match[0];
    if (token.startsWith('`') && token.endsWith('`')) {
      parts.push(
        <code
          key={key++}
          className="px-1 py-0.5 mx-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)]/60 font-mono text-[11.5px] text-[var(--text-primary)]"
        >
          {token.slice(1, -1)}
        </code>
      );
    } else if (token.startsWith('**') && token.endsWith('**')) {
      parts.push(
        <strong key={key++} className="font-semibold text-[var(--text-primary)]">
          {token.slice(2, -2)}
        </strong>
      );
    } else if (token.startsWith('*') && token.endsWith('*')) {
      parts.push(
        <em key={key++} className="italic">
          {token.slice(1, -1)}
        </em>
      );
    } else if (token.startsWith('[')) {
      const linkMatch = /^\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)$/.exec(token);
      if (linkMatch) {
        parts.push(
          <a
            key={key++}
            href={linkMatch[2]}
            target="_blank"
            rel="noreferrer"
            className="text-[var(--accent)] hover:underline"
          >
            {linkMatch[1]}
          </a>
        );
      } else {
        parts.push(token);
      }
    }
    lastIndex = regex.lastIndex;
  }

  if (lastIndex < text.length) {
    parts.push(text.slice(lastIndex));
  }
  return parts;
}

const MarkdownProse: React.FC<{ text: string }> = ({ text }) => {
  const lines = text.split(/\r?\n/);
  const blocks: React.ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }

    // Heading
    const headingMatch = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (headingMatch) {
      const level = headingMatch[1].length;
      const cls =
        level === 1
          ? 'text-[13.5px] font-semibold mt-2.5 mb-1 text-[var(--text-primary)]'
          : level === 2
          ? 'text-[13px] font-semibold mt-2 mb-1 text-[var(--text-primary)]'
          : level === 3
          ? 'text-[12.5px] font-semibold mt-1.5 mb-0.5 text-[var(--text-primary)]'
          : 'text-[12.5px] font-medium mt-1.5 mb-0.5 text-[var(--text-muted)]';
      blocks.push(
        <div key={key++} className={cls}>
          {renderInlineMarkdown(headingMatch[2])}
        </div>
      );
      i++;
      continue;
    }

    // Horizontal rule
    if (/^(-{3,}|\*{3,})$/.test(line.trim())) {
      blocks.push(<hr key={key++} className="my-2 border-[var(--border)]" />);
      i++;
      continue;
    }

    // Blockquote
    if (line.startsWith('>')) {
      const qLines: string[] = [];
      while (i < lines.length && lines[i].startsWith('>')) {
        qLines.push(lines[i].replace(/^>\s?/, ''));
        i++;
      }
      blocks.push(
        <blockquote
          key={key++}
          className="my-1.5 pl-2.5 border-l-2 border-[var(--accent)]/70 text-[var(--text-muted)] space-y-0.5"
        >
          {qLines.map((ql, idx) => (
            <div key={idx}>{renderInlineMarkdown(ql)}</div>
          ))}
        </blockquote>
      );
      continue;
    }

    // Unordered list
    if (/^\s*[-*]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*]\s+/, ''));
        i++;
      }
      blocks.push(
        <ul key={key++} className="my-1.5 pl-4 list-disc space-y-0.5">
          {items.map((it, idx) => (
            <li key={idx} className="leading-[1.55]">
              {renderInlineMarkdown(it)}
            </li>
          ))}
        </ul>
      );
      continue;
    }

    // Ordered list
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ''));
        i++;
      }
      blocks.push(
        <ol key={key++} className="my-1.5 pl-4 list-decimal space-y-0.5">
          {items.map((it, idx) => (
            <li key={idx} className="leading-[1.55]">
              {renderInlineMarkdown(it)}
            </li>
          ))}
        </ol>
      );
      continue;
    }

    // Table (header row + delimiter row)
    if (
      line.includes('|') &&
      i + 1 < lines.length &&
      /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(lines[i + 1])
    ) {
      const splitRow = (row: string) =>
        row.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
      const header = splitRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim() !== '') {
        rows.push(splitRow(lines[i]));
        i++;
      }
      blocks.push(
        <div key={key++} className="my-2 overflow-x-auto">
          <table className="border-collapse text-[12.5px]">
            <thead>
              <tr>
                {header.map((h, idx) => (
                  <th
                    key={idx}
                    className="border border-[var(--border)] px-2 py-1 text-left font-semibold bg-[var(--bg-elevated)]"
                  >
                    {renderInlineMarkdown(h)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r, ri) => (
                <tr key={ri}>
                  {header.map((_, ci) => (
                    <td key={ci} className="border border-[var(--border)] px-2 py-1">
                      {renderInlineMarkdown(r[ci] ?? '')}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
      continue;
    }

    // Regular paragraph (consume consecutive plain lines)
    const pLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== '' &&
      !/^(#{1,6}\s+|>|\s*[-*]\s+|\s*\d+\.\s+|(-{3,}|\*{3,})$)/.test(lines[i])
    ) {
      pLines.push(lines[i]);
      i++;
    }
    blocks.push(
      <p key={key++} className="my-1.5 first:mt-0 last:mb-0 leading-[1.6]">
        {pLines.map((pl, idx) => (
          <React.Fragment key={idx}>
            {idx > 0 && <br />}
            {renderInlineMarkdown(pl)}
          </React.Fragment>
        ))}
      </p>
    );
  }

  return <>{blocks}</>;
};

export const MarkdownMessage: React.FC<{ content: string; theme: LiteTheme }> = ({
  content,
  theme,
}) => {
  // Split into prose and fenced code segments (tolerates unclosed fence during streaming)
  const segments: Array<
    { type: 'prose'; text: string } | { type: 'code'; lang: string; code: string }
  > = [];

  const fenceRegex = /```([a-zA-Z0-9_+-]*)[ \t]*\r?\n([\s\S]*?)(?:```|$)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = fenceRegex.exec(content)) !== null) {
    if (match.index > lastIndex) {
      segments.push({ type: 'prose', text: content.slice(lastIndex, match.index) });
    }
    const rawCode = match[2].replace(/\r?\n$/, '');
    segments.push({ type: 'code', lang: match[1] || '', code: rawCode });
    lastIndex = fenceRegex.lastIndex;
  }

  if (lastIndex < content.length) {
    segments.push({ type: 'prose', text: content.slice(lastIndex) });
  }

  return (
    <div className="text-[13.5px] leading-[1.65] text-[var(--text-primary)] break-words">
      {segments.map((seg, idx) =>
        seg.type === 'code' ? (
          <FencedCodeBlock key={idx} lang={seg.lang} code={seg.code} theme={theme} />
        ) : (
          <MarkdownProse key={idx} text={seg.text} />
        )
      )}
    </div>
  );
};
