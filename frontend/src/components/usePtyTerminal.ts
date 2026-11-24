import { useEffect, useRef, useSyncExternalStore } from 'react';
import type { RefObject } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { wsUrl } from '../api';
import { isUsableDrag, readDrop, shellQuote, uploadDropped } from '../utils/dnd';
import type { LiteTheme } from '../themes';

const PRIMARY_FONT = 'JetBrains Mono Variable';
const FONT_FAMILY = `'${PRIMARY_FONT}', 'JetBrains Mono', 'Cascadia Code', Consolas, monospace`;
const IS_WINDOWS = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent);

/**
 * Debounce for the PTY resize only (ConPTY redraws the whole screen on every resize). The local
 * grid is re-fitted every frame so the terminal follows a dragged splitter smoothly.
 */
const PTY_RESIZE_DEBOUNCE_MS = 90;
/** Never push a smaller grid than this to the PTY; a collapsed/hidden host measures ~2x1. */
const MIN_COLS = 10;
const MIN_ROWS = 3;
/** Output bytes the terminal must finish writing before we ACK them (server pauses at 512 KiB in flight). */
const ACK_STEP = 128 * 1024;
const MAX_RECONNECTS = 6;

// ---------------------------------------------------------------------------
// Engine selection (persisted). xterm.js + WebGL is the default; ghostty-web
// (libghostty-vt compiled to WASM) is available as an opt-in engine.
// ---------------------------------------------------------------------------

export type TerminalEngine = 'xterm' | 'ghostty';
const ENGINE_KEY = 'vireo.terminalEngine';
const engineListeners = new Set<() => void>();

function readEngine(): TerminalEngine {
  try {
    return localStorage.getItem(ENGINE_KEY) === 'ghostty' ? 'ghostty' : 'xterm';
  } catch {
    return 'xterm';
  }
}

export function setTerminalEngine(engine: TerminalEngine): void {
  try {
    localStorage.setItem(ENGINE_KEY, engine);
  } catch {
    // storage unavailable: the choice just won't persist
  }
  engineListeners.forEach((l) => l());
}

export function useTerminalEngine(): [TerminalEngine, (e: TerminalEngine) => void] {
  const engine = useSyncExternalStore(
    (cb) => {
      engineListeners.add(cb);
      window.addEventListener('storage', cb);
      return () => {
        engineListeners.delete(cb);
        window.removeEventListener('storage', cb);
      };
    },
    readEngine,
    () => 'xterm' as TerminalEngine
  );
  return [engine, setTerminalEngine];
}

/** End a persistent PTY session on the backend (tab closed, agent relaunched, ...). */
export function killPtySession(sid: string): void {
  fetch(`/api/pty/kill?sid=${encodeURIComponent(sid)}`, { method: 'POST', keepalive: true }).catch(
    () => {}
  );
}

export interface PtyTerminalOptions {
  /**
   * Stable id of the backend shell. The same id re-attaches to the running shell (page reload,
   * dropped socket, panel remount, engine switch); a new id starts a fresh shell and kills the
   * previous one.
   */
  sessionKey: string;
  cmd?: string;
  /** Working directory relative to the workspace root. */
  cwd?: string;
  /** Extra environment variables for the spawned shell. */
  env?: Record<string, string>;
  active: boolean;
  /** False while the whole terminal dock is collapsed (tabs stay mounted but must go idle). */
  visible?: boolean;
  theme: LiteTheme;
  fontSize: number;
  scrollback: number;
  exitMessage: string;
  focusOnOpen?: boolean;
}

/** Clean plain-text tail of the terminal, read from the parsed buffer (no ANSI). */
export function readTerminalTail(term: Terminal, maxLines = 150): string {
  const buf = term.buffer.active;
  const end = buf.length;
  const lines: string[] = [];
  for (let i = Math.max(0, end - maxLines); i < end; i++) {
    const line = buf.getLine(i);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && lines.length > 0) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').trimEnd();
}

async function waitForFont(fontSize: number): Promise<void> {
  if (typeof document === 'undefined' || !document.fonts) return;
  const loaded = Promise.all([
    document.fonts.load(`${fontSize}px '${PRIMARY_FONT}'`),
    document.fonts.ready,
  ]).then(() => undefined);
  const timeout = new Promise<void>((resolve) => window.setTimeout(resolve, 1500));
  try {
    await Promise.race([loaded, timeout]);
  } catch {
    // font loading is best-effort
  }
}

interface FitLike {
  proposeDimensions(): { cols: number; rows: number } | undefined;
}

interface Built {
  term: Terminal;
  fit: FitLike;
  engine: TerminalEngine;
}

function buildXterm(opts: PtyTerminalOptions, host: HTMLElement): Built {
  const term = new Terminal({
    cursorBlink: false,
    fontSize: opts.fontSize,
    lineHeight: 1.38,
    fontFamily: FONT_FAMILY,
    theme: opts.theme.terminal,
    scrollback: opts.scrollback,
    allowProposedApi: true,
    ...(IS_WINDOWS ? { windowsPty: { backend: 'conpty' as const, buildNumber: 22621 } } : {}),
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new WebLinksAddon());
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = '11';
  term.open(host);
  return { term, fit, engine: 'xterm' };
}

async function buildGhostty(opts: PtyTerminalOptions, host: HTMLElement): Promise<Built> {
  const g = await import('ghostty-web');
  await g.init();
  const term = new g.Terminal({
    cursorBlink: false,
    fontSize: opts.fontSize,
    fontFamily: FONT_FAMILY,
    theme: opts.theme.terminal,
    scrollback: opts.scrollback,
  });
  const fit = new g.FitAddon();
  term.loadAddon(fit);
  term.open(host);
  // ghostty-web makes the host contenteditable (for IME/paste), so the browser draws its own
  // text caret there on top of the canvas cursor: the "second cursor".
  host.style.caretColor = 'transparent';
  host.style.outline = 'none';
  return { term: term as unknown as Terminal, fit, engine: 'ghostty' };
}

/**
 * Shared terminal <-> /ws/pty wiring for the bottom terminal and the CLI agent pane.
 *
 * Client: WebGL rendering + Unicode 11 (xterm engine), debounced/deduped PTY resizes that never
 * fire while hidden, bracketed paste, binary output frames, ACK flow control, automatic
 * reconnect with screen replay (terminal replies muted while replaying).
 */
export function usePtyTerminal(
  hostRef: RefObject<HTMLDivElement | null>,
  options: PtyTerminalOptions
) {
  const [engine] = useTerminalEngine();
  const termRef = useRef<Terminal | null>(null);
  const controlRef = useRef<{ refit: () => void; sync: () => void } | null>(null);
  const optsRef = useRef(options);
  optsRef.current = options;
  const prevKeyRef = useRef<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let teardown: (() => void) | null = null;
    const opts = optsRef.current;
    const { fontSize, cmd, exitMessage, sessionKey } = opts;

    // The session this hook used to own is no longer wanted (new workspace / relaunch).
    if (prevKeyRef.current && prevKeyRef.current !== sessionKey) killPtySession(prevKeyRef.current);
    prevKeyRef.current = sessionKey;

    const start = async () => {
      let built: Built;
      try {
        built = engine === 'ghostty' ? await buildGhostty(opts, host) : buildXterm(opts, host);
      } catch (err) {
        console.warn('Falling back to xterm.js:', err);
        host.replaceChildren();
        built = buildXterm(opts, host);
      }
      const { term, fit } = built;
      if (disposed) {
        term.dispose();
        return;
      }
      termRef.current = term;
      // ghostty-web parses synchronously (its write callback is a rAF that stops in the
      // background), so for it "written" means "handed to write()".
      const syncWrites = built.engine === 'ghostty';

      // WebGL only while this tab is the visible one: every context pins GPU-process memory and
      // keeps the compositor repainting. Disposed when hidden, re-attached on demand.
      const wantLive = () => !document.hidden && optsRef.current.active && optsRef.current.visible !== false;
      const wantWebgl = () => optsRef.current.active && optsRef.current.visible !== false;
      let webglAddon: { dispose: () => void } | null = null;
      let webglLoading = false;
      let webglBroken = built.engine !== 'xterm';
      const tryWebgl = () => {
        if (webglBroken || webglLoading || webglAddon || disposed || !wantWebgl()) return;
        webglLoading = true;
        // Loaded on demand so the WebGL renderer is not part of the base terminal chunk.
        import('@xterm/addon-webgl')
          .then(({ WebglAddon }) => {
            webglLoading = false;
            if (disposed || !wantWebgl()) return;
            const addon = new WebglAddon();
            addon.onContextLoss(() => {
              addon.dispose();
              if (webglAddon === addon) webglAddon = null;
              webglBroken = true;
            });
            term.loadAddon(addon);
            webglAddon = addon;
          })
          .catch(() => {
            webglLoading = false;
            webglBroken = true; // WebGL unavailable: keep the DOM renderer
          });
      };
      /** Apply visibility-dependent settings: cursor blink timer + WebGL context. */
      const sync = () => {
        try {
          const blink = wantLive();
          if (term.options.cursorBlink !== blink) term.options.cursorBlink = blink;
        } catch {
          // some engines cannot change options live
        }
        if (wantWebgl()) {
          if (host.clientWidth >= 40) tryWebgl();
        } else if (webglAddon) {
          try {
            webglAddon.dispose();
          } catch {
            // already disposed by context loss
          }
          webglAddon = null;
        }
      };
      const onVisibility = () => sync();
      document.addEventListener('visibilitychange', onVisibility);

      let sock: WebSocket | null = null;
      let lastSent = '';
      const sendResize = () => {
        if (!sock || sock.readyState !== WebSocket.OPEN) return;
        const key = `${term.cols}x${term.rows}`;
        if (key === lastSent) return;
        lastSent = key;
        sock.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
      };

      /** Fit to the host. Returns false (and changes nothing) while hidden or collapsed. */
      const fitNow = (): boolean => {
        if (disposed || host.clientWidth < 40 || host.clientHeight < 30) return false;
        try {
          const dims = fit.proposeDimensions();
          if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return false;
          if (dims.cols < MIN_COLS || dims.rows < MIN_ROWS) return false;
          if (dims.cols !== term.cols || dims.rows !== term.rows) term.resize(dims.cols, dims.rows);
          return true;
        } catch {
          return false;
        }
      };

      let resizeTimer = 0;
      let fitFrame = 0;
      const schedulePtyResize = () => {
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(sendResize, PTY_RESIZE_DEBOUNCE_MS);
      };
      // At most one fit per frame: reflow locally right away, tell the PTY once things settle.
      const refit = () => {
        if (fitFrame) return;
        fitFrame = requestAnimationFrame(() => {
          fitFrame = 0;
          if (fitNow()) {
            sync();
            schedulePtyResize();
          }
        });
      };
      controlRef.current = { refit, sync };

      // Return value means "let the terminal process this key" for xterm.js, but "I handled
      // it, skip default handling" for ghostty-web, so the two are inverted.
      const HANDLED = built.engine === 'ghostty';
      const sendInput = (data: string) => {
        if (!disposed && sock && sock.readyState === WebSocket.OPEN) {
          sock.send(JSON.stringify({ type: 'input', data }));
        }
      };
      term.attachCustomKeyEventHandler((ev) => {
        // The terminal encodes Shift+Enter as a plain CR, so CLI agents (Claude Code, Codex)
        // submit instead of starting a new line. Send LF (Ctrl+J) instead: those CLIs treat it
        // as "insert newline", and plain shells treat it like Enter, as before. Every event
        // phase is swallowed so the keypress can't still emit a CR.
        if (ev.key === 'Enter' && ev.shiftKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
          ev.preventDefault();
          if (ev.type === 'keydown') sendInput('\n');
          return HANDLED;
        }
        if (ev.type !== 'keydown') return !HANDLED;
        if (!(ev.ctrlKey || ev.metaKey) || ev.altKey) return !HANDLED;
        const key = ev.key.toLowerCase();
        if (key === 'c' && term.hasSelection()) {
          const sel = term.getSelection();
          if (sel) {
            navigator.clipboard?.writeText(sel).catch(() => {});
            term.clearSelection();
          }
          return HANDLED;
        }
        if (key === 'v') {
          // Must report "not handled" for both engines: ghostty-web preventDefault()s any key
          // its handler claims, which would cancel the native `paste` event (handled below) that
          // carries the clipboard without the permission prompt navigator.clipboard.read* shows.
          return false;
        }
        return !HANDLED;
      });

      // term.paste honours bracketed-paste mode; xterm.js also normalises newlines to CR,
      // ghostty-web does not, so do it here for both (idempotent).
      const pasteText = (text: string) => {
        if (disposed || !text) return;
        term.paste(text.replace(/\r?\n/g, '\r'));
      };
      // Image on the clipboard (no text): the browser already holds the bytes, so save them
      // straight to disk through the local backend and paste the path. CLI agents treat a pasted
      // image path as an attachment. This skips the agent's own (slow) OS-clipboard read via a
      // spawned PowerShell; that key remains the fallback if the upload fails.
      const pasteImageKey = IS_WINDOWS ? '\x1bv' : '\x16'; // Alt+V on Windows, Ctrl+V elsewhere
      const onPaste = (ev: ClipboardEvent) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (disposed) return;
        const dt = ev.clipboardData;
        const text = dt?.getData('text/plain');
        if (text) return pasteText(text);
        const images = Array.from(dt?.files ?? []).filter((f) => f.type.startsWith('image/'));
        if (images.length === 0) return sendInput(pasteImageKey);
        Promise.allSettled(images.map(uploadDropped)).then((res) => {
          const saved = res.flatMap((r) => (r.status === 'fulfilled' ? [r.value] : []));
          if (saved.length > 0) pasteText(saved.map(shellQuote).join(' ') + ' ');
          else if (!disposed) sendInput(pasteImageKey);
        });
      };
      host.addEventListener('paste', onPaste, true);

      // Drag-and-drop: Vireo files paste their absolute path; OS files are saved to a temp
      // folder first (browsers hide real paths) and that path is pasted. Works for plain shells
      // and for CLI agents (they read pasted paths as file/image references).
      const onDragOver = (ev: DragEvent) => {
        if (!isUsableDrag(ev.dataTransfer)) return;
        ev.preventDefault();
        if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'copy';
      };
      const onDrop = (ev: DragEvent) => {
        if (!isUsableDrag(ev.dataTransfer)) return;
        ev.preventDefault();
        ev.stopPropagation();
        const { paths, files } = readDrop(ev.dataTransfer);
        const out = paths.map((p) => p.abs);
        const paste = () => {
          if (disposed || out.length === 0) return;
          term.paste(out.map(shellQuote).join(' ') + ' ');
          term.focus();
        };
        if (files.length === 0) return paste();
        Promise.allSettled(files.map(uploadDropped)).then((res) => {
          for (const r of res) if (r.status === 'fulfilled') out.push(r.value);
          paste();
        });
      };
      host.addEventListener('dragover', onDragOver);
      host.addEventListener('drop', onDrop);

      // Replayed history can contain terminal queries; the emulator would answer them into the
      // live shell, so its output is muted until the server says the replay has been written.
      let replaying = true;
      const dataSub = term.onData((data) => {
        if (!disposed && !replaying && sock && sock.readyState === WebSocket.OPEN) {
          sock.send(JSON.stringify({ type: 'input', data }));
        }
      });

      let attempt = 0;
      let reconnectTimer = 0;
      let ackTimer = 0;
      // The server replays the screen history on every (re)connect. Painting that replay shows a
      // blank screen and then a scroll-through (a visible flash), so keep the host invisible until
      // the replay has been parsed. The timer is a safety net in case "ready" never arrives.
      let revealTimer = 0;
      const reveal = () => {
        window.clearTimeout(revealTimer);
        revealTimer = 0;
        host.style.visibility = '';
      };
      const hideForReplay = () => {
        host.style.visibility = 'hidden';
        window.clearTimeout(revealTimer);
        revealTimer = window.setTimeout(reveal, 1500);
      };
      const connect = () => {
        const cols = Math.max(20, term.cols);
        const rows = Math.max(5, term.rows);
        const params = new URLSearchParams({
          cols: String(cols),
          rows: String(rows),
          sid: sessionKey,
        });
        if (cmd) params.set('cmd', cmd);
        if (options.cwd) params.set('cwd', options.cwd);
        if (options.env && Object.keys(options.env).length > 0)
          params.set('env', JSON.stringify(options.env));
        const s = new WebSocket(wsUrl(`/ws/pty?${params.toString()}`));
        s.binaryType = 'arraybuffer';
        sock = s;
        lastSent = ''; // always tell the (possibly pre-existing) shell our real size once
        replaying = true;
        hideForReplay();
        let written = 0;
        let acked = 0;
        const sendAck = () => {
          if (s.readyState === WebSocket.OPEN && written > acked) {
            acked = written;
            s.send(JSON.stringify({ type: 'ack', bytes: written }));
          }
        };

        s.onopen = () => {
          if (disposed) return;
          if (attempt > 0) term.reset(); // the server replays the whole screen history
          attempt = 0;
          sendResize();
          if (optsRef.current.focusOnOpen) term.focus();
        };
        s.onmessage = (ev) => {
          if (disposed) return;
          if (typeof ev.data === 'string') {
            if (ev.data.startsWith('{"type":"ready"')) {
              if (syncWrites) {
                replaying = false;
                requestAnimationFrame(reveal);
              } else {
                // writes are ordered: this callback runs after all replayed bytes were parsed
                term.write('', () => {
                  replaying = false;
                  requestAnimationFrame(reveal);
                });
              }
            } else {
              term.write(ev.data);
            }
            return;
          }
          const bytes = new Uint8Array(ev.data as ArrayBuffer);
          if (syncWrites) {
            term.write(bytes);
            written += bytes.length;
            if (written - acked >= ACK_STEP) sendAck();
          } else {
            term.write(bytes, () => {
              written += bytes.length;
              if (written - acked >= ACK_STEP) sendAck();
            });
          }
          // small bursts never reach ACK_STEP; flush them shortly so the server window reopens
          if (!ackTimer) {
            ackTimer = window.setTimeout(() => {
              ackTimer = 0;
              sendAck();
            }, 50);
          }
        };
        s.onclose = (ev) => {
          if (disposed || sock !== s) return;
          reveal();
          if (ev.code === 1000) {
            term.write('\r\n\x1b[90m' + exitMessage + '\x1b[0m\r\n');
            return;
          }
          if (attempt >= MAX_RECONNECTS) {
            term.write('\r\n\x1b[31m[Disconnected from backend]\x1b[0m\r\n');
            return;
          }
          attempt += 1;
          reconnectTimer = window.setTimeout(connect, Math.min(4000, 300 * 2 ** attempt));
        };
      };

      const ro = new ResizeObserver(refit);
      ro.observe(host);

      teardown = () => {
        window.clearTimeout(resizeTimer);
        cancelAnimationFrame(fitFrame);
        window.clearTimeout(revealTimer);
        host.style.visibility = '';
        window.clearTimeout(reconnectTimer);
        window.clearTimeout(ackTimer);
        document.removeEventListener('visibilitychange', onVisibility);
        ro.disconnect();
        dataSub.dispose();
        host.removeEventListener('paste', onPaste, true);
        host.removeEventListener('dragover', onDragOver);
        host.removeEventListener('drop', onDrop);
        if (sock) sock.close();
        controlRef.current = null;
        termRef.current = null;
        term.dispose();
      };

      // Measure with the real font and connect only once the grid size is known.
      await waitForFont(fontSize);
      if (disposed) return;
      fitNow();
      sync();
      connect();
    };

    void start();

    return () => {
      disposed = true;
      teardown?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options.sessionKey, engine]);

  useEffect(() => {
    if (!termRef.current) return;
    try {
      termRef.current.options.theme = options.theme.terminal;
    } catch {
      // some engines cannot retheme a live terminal
    }
  }, [options.theme]);

  useEffect(() => {
    if (!options.active) return;
    controlRef.current?.refit();
    const t = window.setTimeout(() => termRef.current?.focus(), 30);
    return () => window.clearTimeout(t);
  }, [options.active]);

  const live = options.active && options.visible !== false;
  useEffect(() => {
    controlRef.current?.sync();
    if (live) controlRef.current?.refit();
  }, [live]);

  return { termRef };
}
