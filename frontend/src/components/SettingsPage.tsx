import React, { useEffect } from 'react';
import { Check, RefreshCw } from 'lucide-react';
import {
  API_PROVIDERS,
  CLI_PROVIDERS,
  CLI_PROVIDER_TOOL,
  PERM_MODES,
  PROVIDER_LABEL,
  useAgentSettings,
} from '../agentSettings';
import type { PermissionMode } from '../types';

const OK_DOT = '#10b981';
const WARN_DOT = '#f59e0b';

const inputCls =
  'w-full h-8 px-2.5 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)] text-ellipsis';

const Section: React.FC<{ title: string; hint?: string; children: React.ReactNode }> = ({
  title,
  hint,
  children,
}) => (
  <section className="mb-7">
    <h2 className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">{title}</h2>
    {hint && <p className="mt-0.5 text-[12px] text-[var(--text-dim)]">{hint}</p>}
    <div className="mt-2.5 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)]/40 divide-y divide-[var(--border)]">
      {children}
    </div>
  </section>
);

const Row: React.FC<{
  label: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}> = ({ label, description, children }) => (
  <div className="flex items-start justify-between gap-6 px-3.5 py-3">
    <div className="min-w-0 max-w-[48%]">
      <div className="text-[13px] text-[var(--text-primary)]">{label}</div>
      {description && <div className="mt-0.5 text-[12px] leading-snug text-[var(--text-dim)]">{description}</div>}
    </div>
    <div className="w-[52%] min-w-0 flex justify-end">{children}</div>
  </div>
);

const Toggle: React.FC<{ checked: boolean; onChange: (v: boolean) => void; label: string }> = ({
  checked,
  onChange,
  label,
}) => (
  <button
    role="switch"
    aria-checked={checked}
    aria-label={label}
    onClick={() => onChange(!checked)}
    className={`relative w-8 h-[18px] rounded-full shrink-0 cursor-pointer transition-colors ${
      checked ? 'bg-[var(--accent)]' : 'bg-[var(--border)]'
    }`}
  >
    <span
      className={`absolute top-[2px] left-[2px] w-[14px] h-[14px] rounded-full bg-white transition-transform ${
        checked ? 'translate-x-[14px]' : ''
      }`}
    />
  </button>
);

export const SettingsPage: React.FC = () => {
  const {
    config,
    setConfig,
    setProvider,
    cliTools,
    localModels,
    modelOptions,
    apiModelsError,
    refresh,
    showQuickActions,
    setShowQuickActions,
  } = useAgentSettings();

  // Re-detect installed engines every time the page is opened.
  useEffect(() => {
    refresh();
  }, [refresh]);

  const toolId = CLI_PROVIDER_TOOL[config.provider];
  const tool = toolId ? cliTools.find((t) => t.id === toolId) : undefined;
  const isCli = !!toolId;
  const isApi = API_PROVIDERS.includes(config.provider);
  const isOllama = config.provider === 'ollama';
  const permMode = config.permissionMode ?? 'auto_edit';

  let status: { ok: boolean; text: string } | null = null;
  if (isCli) {
    status = tool
      ? tool.installed
        ? { ok: true, text: `Installed${tool.path ? ` · ${tool.path}` : ''}` }
        : { ok: false, text: `${PROVIDER_LABEL[config.provider]} CLI not found. ${tool.install_hint}` }
      : null;
  } else if (isOllama) {
    status = localModels
      ? localModels.ollama.running
        ? { ok: true, text: `Running · ${localModels.ollama.models.length} models` }
        : { ok: false, text: 'Ollama is not running at this address.' }
      : null;
  } else {
    status = config.apiKey.trim()
      ? apiModelsError
        ? { ok: false, text: apiModelsError }
        : { ok: true, text: 'API key set' }
      : { ok: false, text: 'Add an API key to connect.' };
  }

  const permNote = (mode: PermissionMode) =>
    config.provider === 'claude-cli'
      ? PERM_MODES[mode].claudeNote
      : config.provider === 'codex-cli'
        ? PERM_MODES[mode].codexNote
        : PERM_MODES[mode].cliNote;

  return (
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)]">
      <div className="mx-auto w-full max-w-[720px] px-6 py-6">
        <h1 className="text-[18px] font-semibold text-[var(--text-primary)]">Settings</h1>
        <p className="mt-1 mb-6 text-[12.5px] text-[var(--text-dim)]">
          Engine, models and preferences for the agent panel.
        </p>

        <Section title="Engine" hint="Run an installed CLI agent with its own login, or connect an API / local model.">
          <Row label="Provider" description={isCli ? 'Uses the CLI’s own login, no API key.' : undefined}>
            <select
              value={config.provider}
              onChange={(e) => setProvider(e.target.value as typeof config.provider)}
              className={`${inputCls} cursor-pointer`}
            >
              <optgroup label="CLI agents">
                {CLI_PROVIDERS.map((p) => {
                  const t = cliTools.find((x) => x.id === CLI_PROVIDER_TOOL[p]);
                  return (
                    <option key={p} value={p}>
                      {PROVIDER_LABEL[p]} CLI{t && !t.installed ? ' (not installed)' : ''}
                    </option>
                  );
                })}
              </optgroup>
              <optgroup label="API & local">
                <option value="anthropic">Anthropic API</option>
                <option value="openai">OpenAI / compatible API</option>
                <option value="gemini">Google Gemini API</option>
                <option value="ollama">Ollama (local)</option>
              </optgroup>
            </select>
          </Row>

          {status && (
            <div className="flex items-center gap-2 px-3.5 py-2 text-[12px]">
              <span
                className="w-1.5 h-1.5 rounded-full shrink-0"
                style={{ background: status.ok ? OK_DOT : WARN_DOT }}
              />
              <span className="truncate" style={{ color: status.ok ? OK_DOT : WARN_DOT }} title={status.text}>
                {status.text}
              </span>
            </div>
          )}

          <Row
            label="Model"
            description={
              isCli
                ? '“default” keeps the CLI’s configured model. Listed models come from the CLI itself.'
                : isApi
                  ? 'Loaded live from the provider.'
                  : undefined
            }
          >
            <div className="flex w-full items-center gap-1.5">
              <input
                list="vireo-model-options"
                value={config.model}
                onChange={(e) => setConfig((prev) => ({ ...prev, model: e.target.value }))}
                placeholder={isApi ? 'Add a key to list models, or type an id' : 'Model id'}
                spellCheck={false}
                className={`${inputCls} font-mono text-[12px]`}
              />
              <datalist id="vireo-model-options">
                {modelOptions.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
              <button
                onClick={refresh}
                title="Refresh installed engines and models"
                className="h-8 w-8 shrink-0 flex items-center justify-center rounded-md border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
              >
                <RefreshCw size={13} />
              </button>
            </div>
          </Row>

          {modelOptions.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-3.5 py-2.5">
              {modelOptions.map((m) => (
                <button
                  key={m}
                  onClick={() => setConfig((prev) => ({ ...prev, model: m }))}
                  className={`inline-flex items-center gap-1 h-6 max-w-full px-2 rounded-md border text-[11.5px] font-mono cursor-pointer transition-colors ${
                    config.model === m
                      ? 'border-[var(--accent)] text-[var(--text-primary)] bg-[var(--bg-hover)]'
                      : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {config.model === m && <Check size={10} className="text-[var(--accent)] shrink-0" />}
                  <span className="truncate">{m}</span>
                </button>
              ))}
            </div>
          )}

          {(isApi || isOllama) && config.provider !== 'gemini' && (
            <Row
              label="Base URL"
              description={isOllama ? 'Where Ollama is listening.' : 'Optional. Point at OpenRouter, DeepSeek, LM Studio or any compatible endpoint.'}
            >
              <input
                value={config.baseUrl}
                onChange={(e) => setConfig((prev) => ({ ...prev, baseUrl: e.target.value }))}
                placeholder={isOllama ? 'http://127.0.0.1:11434/v1' : 'https://api.openai.com/v1'}
                spellCheck={false}
                className={inputCls}
              />
            </Row>
          )}

          {isApi && (
            <Row label="API key" description="Saved in your OS credential store, not in the browser.">
              <input
                type="password"
                value={config.apiKey}
                onChange={(e) => setConfig((prev) => ({ ...prev, apiKey: e.target.value }))}
                placeholder="sk-…"
                autoComplete="off"
                className={inputCls}
              />
            </Row>
          )}
        </Section>

        {isCli && (
          <Section title="Permissions" hint="What the agent may do without asking.">
            {(Object.keys(PERM_MODES) as PermissionMode[]).map((mode) => {
              const m = PERM_MODES[mode];
              const Icon = m.icon;
              const active = permMode === mode;
              return (
                <button
                  key={mode}
                  onClick={() => setConfig((prev) => ({ ...prev, permissionMode: mode }))}
                  className="w-full flex items-start gap-3 px-3.5 py-2.5 text-left cursor-pointer hover:bg-[var(--bg-hover)] transition-colors"
                >
                  <Icon size={14} className="mt-0.5 shrink-0 text-[var(--text-muted)]" />
                  <div className="flex-1 min-w-0">
                    <div className="text-[13px] text-[var(--text-primary)]">{m.label}</div>
                    <div className="text-[12px] text-[var(--text-dim)]">{permNote(mode)}</div>
                  </div>
                  {active && <Check size={14} className="mt-0.5 shrink-0 text-[var(--accent)]" />}
                </button>
              );
            })}
          </Section>
        )}

        <Section title="Agent">
          <Row
            label="Checkpoint before agent edits"
            description="Snapshots the workspace first so a turn can be undone."
          >
            <Toggle
              label="Checkpoint before agent edits"
              checked={config.autoCheckpoint}
              onChange={(v) => setConfig((prev) => ({ ...prev, autoCheckpoint: v }))}
            />
          </Row>
          <Row label="Show quick actions" description="Suggested prompts on an empty agent thread.">
            <Toggle label="Show quick actions" checked={showQuickActions} onChange={setShowQuickActions} />
          </Row>
        </Section>
      </div>
    </div>
  );
};
