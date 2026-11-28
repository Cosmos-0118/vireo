import React, { useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkBreaks from 'remark-breaks';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import { Check, Copy } from 'lucide-react';
import type { TokenSpan } from '../syntax';
import type { LiteTheme } from '../themes';

// Stable plugin arrays: fresh literals each render change <ReactMarkdown>'s props every time.
type MdPlugins = React.ComponentProps<typeof ReactMarkdown>['remarkPlugins'];
const DOC_REMARK: MdPlugins = [remarkGfm];
const CHAT_REMARK: MdPlugins = [remarkGfm, remarkBreaks];
const NO_REHYPE: MdPlugins = [];

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
  const lastHighlightRef = useRef(0);
  useEffect(() => {
    if (!normalizedLang) {
      setHighlighted(null);
      return;
    }
    let cancelled = false;
    // While a block streams, `code` changes every frame; a full Lezer parse each time is O(T^2)
    // overall. Cap re-highlighting at ~8/s (the first run is immediate).
    const wait = Math.max(0, lastHighlightRef.current + 120 - Date.now());
    const timer = setTimeout(() => {
      lastHighlightRef.current = Date.now();
      import('../syntax')
        .then(async (m) => {
          await m.loadLanguageByName(normalizedLang);
          if (!cancelled) setHighlighted(m.highlightLinesByName(code, normalizedLang, theme));
        })
        .catch(() => {});
    }, wait);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [code, normalizedLang, theme]);
  // Only the unhighlighted diff fallback reads this.
  const rawLines = useMemo(() => (isDiff ? code.split(/\r?\n/) : []), [isDiff, code]);

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

type Variant = 'chat' | 'document';

const sanitizeSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    '*': [...(defaultSchema.attributes?.['*'] ?? []), 'align'],
    img: [...(defaultSchema.attributes?.img ?? []), 'width', 'height'],
    td: [...(defaultSchema.attributes?.td ?? []), 'align'],
    th: [...(defaultSchema.attributes?.th ?? []), 'align'],
  },
};
const DOC_REHYPE: MdPlugins = [rehypeRaw, [rehypeSanitize, sanitizeSchema]];

const slugify = (text: string) =>
  text
    .toLowerCase()
    .trim()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .replace(/\s/g, '-');

const nodeText = (node: React.ReactNode): string => {
  if (node == null || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join('');
  if (React.isValidElement(node)) return nodeText((node.props as { children?: React.ReactNode }).children);
  return '';
};

/** Resolve `src` relative to the markdown file's folder; null when it leaves the workspace. */
function workspaceImageUrl(src: string, filePath?: string): string | null {
  if (/^(https?:|data:image\/|blob:)/i.test(src)) return src;
  if (!filePath || /^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith('//')) return null;
  let clean = src.split(/[?#]/)[0].replace(/\\/g, '/');
  try {
    clean = decodeURI(clean);
  } catch {
    // keep the raw path
  }
  const parts = clean.startsWith('/') ? [] : filePath.replace(/\\/g, '/').split('/').slice(0, -1);
  for (const seg of clean.split('/')) {
    if (!seg || seg === '.') continue;
    if (seg === '..') {
      if (!parts.length) return null;
      parts.pop();
    } else parts.push(seg);
  }
  return `/api/files/raw?path=${encodeURIComponent(parts.join('/'))}`;
}

type HeadingTag = 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';

function buildComponents(variant: Variant, theme: LiteTheme, filePath?: string): Components {
  const doc = variant === 'document';
  const h = (tag: HeadingTag, cls: string) => {
    const Tag = tag;
    const Heading: React.FC<React.ComponentPropsWithoutRef<'h1'>> = ({ children }) => (
      <Tag id={doc ? slugify(nodeText(children)) : undefined} className={cls}>
        {children}
      </Tag>
    );
    return Heading;
  };
  const strong = 'font-semibold text-[var(--text-primary)]';
  const headings = doc
    ? {
        h1: h('h1', `${strong} text-[28px] leading-[1.25] mt-6 mb-4 pb-2 border-b border-[var(--border)] first:mt-0`),
        h2: h('h2', `${strong} text-[22px] leading-[1.3] mt-7 mb-3 pb-1.5 border-b border-[var(--border)]/70 first:mt-0`),
        h3: h('h3', `${strong} text-[18px] leading-[1.35] mt-6 mb-2 first:mt-0`),
        h4: h('h4', `${strong} text-[15.5px] mt-5 mb-2 first:mt-0`),
        h5: h('h5', `${strong} text-[14px] mt-4 mb-1.5 first:mt-0`),
        h6: h('h6', 'font-semibold text-[13px] mt-4 mb-1.5 text-[var(--text-muted)] first:mt-0'),
      }
    : {
        h1: h('h1', `${strong} text-[15px] mt-3 mb-1 first:mt-0`),
        h2: h('h2', `${strong} text-[14px] mt-2.5 mb-1 first:mt-0`),
        h3: h('h3', `${strong} text-[13.5px] mt-2 mb-0.5 first:mt-0`),
        h4: h('h4', `${strong} text-[13px] mt-2 mb-0.5 first:mt-0`),
        h5: h('h5', 'font-medium text-[13px] mt-1.5 mb-0.5 text-[var(--text-muted)]'),
        h6: h('h6', 'font-medium text-[12.5px] mt-1.5 mb-0.5 text-[var(--text-muted)]'),
      };
  const gap = doc ? 'my-3.5' : 'my-1.5';

  return {
    ...headings,
    p: ({ children }) => <p className={`${gap} first:mt-0 last:mb-0 leading-[1.65]`}>{children}</p>,
    a: ({ href, children }) => {
      if (href?.startsWith('#')) {
        return (
          <a
            href={href}
            className="text-[var(--accent)] hover:underline"
            onClick={(e) => {
              e.preventDefault();
              let id = href.slice(1);
              try {
                id = decodeURIComponent(id);
              } catch {
                // keep the raw id
              }
              const root = (e.currentTarget.closest('[data-md-root]') ?? document) as ParentNode;
              (root.querySelector(`[id="${CSS.escape(id)}"]`) as HTMLElement | null)?.scrollIntoView({
                behavior: 'smooth',
                block: 'start',
              });
            }}
          >
            {children}
          </a>
        );
      }
      if (href && /^(https?:|mailto:)/i.test(href)) {
        return (
          <a href={href} target="_blank" rel="noreferrer noopener" className="text-[var(--accent)] hover:underline">
            {children}
          </a>
        );
      }
      // Relative / unsupported links can't navigate inside a preview: show them as inert links.
      return (
        <a
          title={href}
          className="text-[var(--accent)] hover:underline cursor-default"
          onClick={(e) => e.preventDefault()}
        >
          {children}
        </a>
      );
    },
    strong: ({ children }) => <strong className={strong}>{children}</strong>,
    em: ({ children }) => <em className="italic">{children}</em>,
    del: ({ children }) => <del className="opacity-70">{children}</del>,
    blockquote: ({ children }) => (
      <blockquote
        className={`${gap} pl-3.5 border-l-[3px] border-[var(--border)] text-[var(--text-muted)] [&>*:first-child]:mt-0 [&>*:last-child]:mb-0`}
      >
        {children}
      </blockquote>
    ),
    hr: () => <hr className={`${doc ? 'my-6' : 'my-2'} border-0 border-t border-[var(--border)]`} />,
    ul: ({ children, className }) => (
      <ul
        className={`${gap} space-y-1 [&_ul]:my-1 [&_ol]:my-1 ${
          className?.includes('contains-task-list') ? 'list-none pl-1' : 'pl-6 list-disc'
        }`}
      >
        {children}
      </ul>
    ),
    ol: ({ children }) => <ol className={`${gap} pl-6 list-decimal space-y-1 [&_ul]:my-1 [&_ol]:my-1`}>{children}</ol>,
    li: ({ children, className }) => (
      <li
        className={`leading-[1.6] marker:text-[var(--text-muted)] [&>p]:my-1 ${
          className?.includes('task-list-item') ? 'list-none flex items-start gap-2' : ''
        }`}
      >
        {children}
      </li>
    ),
    input: ({ type, checked }) =>
      type === 'checkbox' ? (
        <input
          type="checkbox"
          checked={!!checked}
          readOnly
          disabled
          className="mt-[0.4em] accent-[var(--accent)] shrink-0"
        />
      ) : null,
    table: ({ children }) => (
      <div className={`${doc ? 'my-4' : 'my-2'} overflow-x-auto`}>
        <table className="border-collapse text-[12.5px] min-w-[50%]">{children}</table>
      </div>
    ),
    th: ({ children, align }) => (
      <th
        style={{ textAlign: align as React.CSSProperties['textAlign'] }}
        className="border border-[var(--border)] px-3 py-1.5 text-left font-semibold bg-[var(--bg-elevated)]"
      >
        {children}
      </th>
    ),
    td: ({ children, align }) => (
      <td
        style={{ textAlign: align as React.CSSProperties['textAlign'] }}
        className="border border-[var(--border)] px-3 py-1.5 align-top"
      >
        {children}
      </td>
    ),
    tr: ({ children }) => <tr className="even:bg-[var(--bg-elevated)]/40">{children}</tr>,
    img: ({ src, alt, width, height }) => {
      const url = typeof src === 'string' ? workspaceImageUrl(src, filePath) : null;
      if (!url) {
        return <span className="text-[var(--text-muted)] italic">{alt ? `[image: ${alt}]` : '[image]'}</span>;
      }
      return (
        <img
          src={url}
          alt={alt ?? ''}
          width={width}
          height={height}
          loading="lazy"
          className="inline-block max-w-full h-auto rounded-sm align-middle"
        />
      );
    },
    details: ({ children }) => (
      <details className={`${gap} rounded-md border border-[var(--border)] px-3 py-1.5`}>{children}</details>
    ),
    summary: ({ children }) => <summary className="cursor-pointer font-medium select-none">{children}</summary>,
    kbd: ({ children }) => (
      <kbd className="px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-elevated)] font-mono text-[11px]">
        {children}
      </kbd>
    ),
    // Block code is rendered by `pre`; this handles inline code only.
    code: ({ children }) => (
      <code className="px-1.5 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)]/60 font-mono text-[0.88em] text-[var(--text-primary)]">
        {children}
      </code>
    ),
    pre: ({ children }) => {
      const child = React.Children.toArray(children)[0];
      if (!React.isValidElement(child)) return <pre>{children}</pre>;
      const props = child.props as { className?: string; children?: React.ReactNode };
      const lang = /language-([\w+#.-]+)/.exec(props.className ?? '')?.[1] ?? '';
      const code = nodeText(props.children).replace(/\r?\n$/, '');
      return <FencedCodeBlock lang={lang} code={code} theme={theme} />;
    },
  };
}

export const MarkdownMessage: React.FC<{
  content: string;
  theme: LiteTheme;
  /** `chat` is compact and keeps single line breaks; `document` is a full GitHub-style render. */
  variant?: Variant;
  /** Workspace-relative path of the markdown file; resolves relative image links. */
  filePath?: string;
}> = React.memo(({ content, theme, variant = 'chat', filePath }) => {
  const components = useMemo(() => buildComponents(variant, theme, filePath), [variant, theme, filePath]);
  const doc = variant === 'document';
  return (
    <div
      data-md-root
      className={`${doc ? 'text-[14.5px] leading-[1.7]' : 'text-[13.5px] leading-[1.65]'} text-[var(--text-primary)] break-words`}
    >
      <ReactMarkdown
        remarkPlugins={doc ? DOC_REMARK : CHAT_REMARK}
        rehypePlugins={doc ? DOC_REHYPE : NO_REHYPE}
        components={components}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
});
MarkdownMessage.displayName = 'MarkdownMessage';
