import React, { useEffect, useMemo, useState } from 'react';
import { Check, Copy, Eye, EyeOff, RefreshCw, X } from 'lucide-react';
import {
  API_PROVIDERS,
  CLI_PROVIDERS,
  CLI_PROVIDER_TOOL,
  PERM_MODES,
  PROVIDER_LABEL,
  PROVIDER_PRESETS,
  useAgentSettings,
  type Provider,
} from '../agentSettings';
import type { PermissionMode } from '../types';
import { Combobox, Select, type SelectOption } from './ui/Select';

const fieldCls =
  'w-full h-8 px-2.5 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[12.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none transition-[border-color,box-shadow] hover:border-[color-mix(in_srgb,var(--text-dim)_60%,var(--border))] focus:border-[var(--accent)] focus:shadow-[0_0_0_2px_color-mix(in_srgb,var(--accent)_18%,transparent)] text-ellipsis';

const Section: React.FC<{
  title: string;
  children: React.ReactNode;
}> = ({ title, children }) => (
  <section className="mb-6 last:mb-0">
    <h2 className="mb-2 px-0.5 text-[11px] font-medium uppercase tracking-wider text-[var(--text-dim)]">
      {title}
    </h2>
    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-panel)] divide-y divide-[var(--border)]/70">
      {children}
    </div>
  </section>
);

const Row: React.FC<{
  label: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}> = ({ label, description, children }) => (
  <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2.5 sm:gap-6 px-4 py-3">
    <div className="min-w-0 sm:max-w-[46%]">
      <div className="text-[13px] font-medium text-[var(--text-primary)] leading-snug">{label}</div>
      {description && (
        <div className="mt-0.5 text-[11.5px] leading-snug text-[var(--text-dim)] truncate">
          {description}
        </div>
      )}
    </div>
    <div className="w-full sm:w-[50%] min-w-0 flex sm:justify-end">{children}</div>
  </div>
);

const Toggle: React.FC<{ checked: boolean; onChange: (v: boolean) => void; label: string }> = ({
  checked,
  onChange,
  label,
}) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    onClick={() => onChange(!checked)}
    className={`relative w-8 h-[18px] rounded-full shrink-0 cursor-pointer transition-colors outline-none focus-visible:shadow-[0_0_0_2px_color-mix(in_srgb,var(--accent)_30%,transparent)] ${
      checked
        ? 'bg-[var(--accent)]'
        : 'bg-[var(--bg-elevated)] border border-[var(--border)] hover:border-[var(--text-dim)]'
    }`}
  >
    <span
      className={`absolute top-1/2 -translate-y-1/2 left-[2px] w-3.5 h-3.5 rounded-full bg-white shadow-sm transition-transform duration-150 ${
        checked ? 'translate-x-[14px]' : 'translate-x-0'
      }`}
    />
  </button>
);

export const SettingsPage: React.FC = () => {
  const {
    config,
    setConfig,
    apiKeys,
    setProvider,
    cliTools,
    localModels,
    modelOptions,
    apiModelsError,
    refresh,
    showQuickActions,
    setShowQuickActions,
  } = useAgentSettings();

  const [showKey, setShowKey] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [copiedHint, setCopiedHint] = useState(false);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    setShowKey(false);
    setCopiedHint(false);
  }, [config.provider]);

  const triggerRefresh = () => {
    refresh();
    setRefreshing(true);
    window.setTimeout(() => setRefreshing(false), 600);
  };

  const toolId = CLI_PROVIDER_TOOL[config.provider];
  const tool = toolId ? cliTools.find((t) => t.id === toolId) : undefined;
  const isCli = !!toolId;
  const isApi = API_PROVIDERS.includes(config.provider);
  const isOllama = config.provider === 'ollama';
  const permMode = config.permissionMode ?? 'auto_edit';

  const providerOptions = useMemo<SelectOption[]>(() => {
    const cliOpts: SelectOption[] = CLI_PROVIDERS.map((p) => {
      const t = cliTools.find((x) => x.id === CLI_PROVIDER_TOOL[p]);
      const installed = t?.installed;
      return {
        value: p,
        label: `${PROVIDER_LABEL[p]} CLI`,
        group: 'CLI',
        dot: installed === true ? 'ok' : installed === false ? 'warn' : 'muted',
      };
    });

    const apiLabels: Record<'anthropic' | 'openai' | 'gemini', string> = {
      anthropic: 'Anthropic API',
      openai: 'OpenAI / Compatible',
      gemini: 'Google Gemini API',
    };

    const apiOpts: SelectOption[] = (['anthropic', 'openai', 'gemini'] as const).map((p) => ({
      value: p,
      label: apiLabels[p],
      group: 'API & Local',
      dot: (apiKeys[p] ?? '').trim() ? 'ok' : 'muted',
    }));

    const ollamaRunning = localModels?.ollama.running;
    const ollamaOpt: SelectOption = {
      value: 'ollama',
      label: 'Ollama',
      group: 'API & Local',
      dot: ollamaRunning === true ? 'ok' : ollamaRunning === false ? 'warn' : 'muted',
    };

    return [...cliOpts, ...apiOpts, ollamaOpt];
  }, [cliTools, apiKeys, localModels]);

  const modelSuggestions = useMemo<SelectOption[]>(
    () => modelOptions.map((m) => ({ value: m, label: m })),
    [modelOptions]
  );

  // Only surface a banner when setup is missing or unreachable; healthy state stays quiet.
  let issue: { message: string; installHint?: string } | null = null;
  let providerSubtitle: React.ReactNode = undefined;

  if (isCli) {
    if (tool?.installed) {
      providerSubtitle = tool.path ? (
        <span className="font-mono text-[11px]" title={tool.path}>
          {tool.path}
        </span>
      ) : (
        'Uses CLI login'
      );
    } else if (tool && !tool.installed) {
      providerSubtitle = 'Not found on PATH';
      issue = {
        message: `${PROVIDER_LABEL[config.provider]} CLI was not found on PATH.`,
        installHint: tool.install_hint || undefined,
      };
    }
  } else if (isOllama) {
    if (localModels?.ollama.running) {
      const n = localModels.ollama.models.length;
      providerSubtitle = `${n} local model${n === 1 ? '' : 's'}`;
    } else if (localModels) {
      providerSubtitle = 'Offline';
      issue = { message: 'Ollama is not reachable at the configured URL.' };
    }
  } else if (apiModelsError) {
    issue = { message: apiModelsError };
  }

  const permNote = (mode: PermissionMode) =>
    config.provider === 'claude-cli'
      ? PERM_MODES[mode].claudeNote
      : config.provider === 'codex-cli'
        ? PERM_MODES[mode].codexNote
        : PERM_MODES[mode].cliNote;

  const defaultBaseUrl = PROVIDER_PRESETS[config.provider].baseUrl;
  const baseUrlModified =
    (isApi || isOllama) && config.provider !== 'gemini' && config.baseUrl.trim() !== defaultBaseUrl;

  const copyInstallHint = (text: string) => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopiedHint(true);
      window.setTimeout(() => setCopiedHint(false), 1400);
    });
  };

  return (
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)]">
      <div className="mx-auto w-full max-w-[640px] px-6 py-8">
        <header className="flex items-center justify-between gap-4 mb-6">
          <h1 className="text-[16px] font-semibold tracking-tight text-[var(--text-primary)]">
            Agent Settings
          </h1>
          <button
            type="button"
            onClick={triggerRefresh}
            title="Re-detect installed CLI tools and refresh models"
            className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
          >
            <RefreshCw size={12} className={refreshing ? 'animate-spin text-[var(--accent)]' : ''} />
            Refresh
          </button>
        </header>

        <Section title="Engine">
          <Row label="Provider" description={providerSubtitle}>
            <Select
              ariaLabel="Provider"
              value={config.provider}
              options={providerOptions}
              onChange={(v) => setProvider(v as Provider)}
            />
          </Row>

          {issue && (
            <div className="px-4 py-2.5 bg-[color-mix(in_srgb,var(--modified-fg)_6%,transparent)] text-[12px] text-[var(--modified-fg)]">
              <p>{issue.message}</p>
              {issue.installHint && (
                <div className="mt-1.5 flex items-center gap-2">
                  <code className="px-2 py-0.5 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[11px] text-[var(--text-primary)] truncate">
                    {issue.installHint}
                  </code>
                  <button
                    type="button"
                    onClick={() => copyInstallHint(issue!.installHint!)}
                    className="shrink-0 inline-flex items-center gap-1 h-6 px-2 rounded border border-[var(--border)] bg-[var(--bg-elevated)] text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer transition-colors"
                  >
                    {copiedHint ? <Check size={11} className="text-[var(--added-fg)]" /> : <Copy size={11} />}
                    {copiedHint ? 'Copied' : 'Copy'}
                  </button>
                </div>
              )}
            </div>
          )}

          <Row label="Model">
            <Combobox
              ariaLabel="Model"
              value={config.model}
              suggestions={modelSuggestions}
              onChange={(v) => setConfig((prev) => ({ ...prev, model: v }))}
              placeholder={isCli ? 'default' : 'Select or type model ID…'}
            />
          </Row>

          {(isApi || isOllama) && config.provider !== 'gemini' && (
            <Row label="Base URL">
              <div className="relative w-full">
                <input
                  value={config.baseUrl}
                  onChange={(e) => setConfig((prev) => ({ ...prev, baseUrl: e.target.value }))}
                  placeholder={isOllama ? 'http://127.0.0.1:11434/v1' : 'https://api.openai.com/v1'}
                  spellCheck={false}
                  className={`${fieldCls} font-mono text-[12px] ${baseUrlModified ? 'pr-14' : ''}`}
                />
                {baseUrlModified && (
                  <button
                    type="button"
                    onClick={() => setConfig((prev) => ({ ...prev, baseUrl: defaultBaseUrl }))}
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 h-5 px-1.5 rounded text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                  >
                    Reset
                  </button>
                )}
              </div>
            </Row>
          )}

          {isApi && (
            <Row label="API key" description="Saved in OS keychain">
              <div className="relative w-full">
                <input
                  type={showKey ? 'text' : 'password'}
                  value={config.apiKey}
                  onChange={(e) => setConfig((prev) => ({ ...prev, apiKey: e.target.value }))}
                  placeholder="sk-…"
                  autoComplete="off"
                  spellCheck={false}
                  className={`${fieldCls} font-mono text-[12px] ${config.apiKey ? 'pr-14' : 'pr-8'}`}
                />
                <div className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center gap-0.5">
                  {config.apiKey && (
                    <button
                      type="button"
                      onClick={() => setConfig((prev) => ({ ...prev, apiKey: '' }))}
                      className="w-6 h-6 rounded flex items-center justify-center text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                      title="Clear"
                      aria-label="Clear API key"
                    >
                      <X size={12} />
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => setShowKey((v) => !v)}
                    className="w-6 h-6 rounded flex items-center justify-center text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    title={showKey ? 'Hide' : 'Reveal'}
                    aria-label={showKey ? 'Hide API key' : 'Reveal API key'}
                  >
                    {showKey ? <EyeOff size={12} /> : <Eye size={12} />}
                  </button>
                </div>
              </div>
            </Row>
          )}

          {isCli && (
            <Row label="Permissions" description={permNote(permMode)}>
              <div
                role="radiogroup"
                aria-label="Permissions"
                className="inline-flex w-full sm:w-auto items-center p-0.5 rounded-lg border border-[var(--border)] bg-[var(--bg-base)]"
              >
                {(Object.keys(PERM_MODES) as PermissionMode[]).map((mode) => {
                  const active = permMode === mode;
                  return (
                    <button
                      key={mode}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      onClick={() => setConfig((prev) => ({ ...prev, permissionMode: mode }))}
                      className={`flex-1 sm:flex-none h-7 px-2.5 rounded-md text-[12px] cursor-pointer transition-colors ${
                        active
                          ? 'bg-[var(--bg-elevated)] text-[var(--text-primary)] font-medium shadow-xs'
                          : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                      }`}
                    >
                      {PERM_MODES[mode].label}
                    </button>
                  );
                })}
              </div>
            </Row>
          )}
        </Section>

        <Section title="Preferences">
          <Row label="Checkpoint before edits" description="Snapshot workspace before each turn for undo">
            <Toggle
              label="Checkpoint before edits"
              checked={config.autoCheckpoint}
              onChange={(v) => setConfig((prev) => ({ ...prev, autoCheckpoint: v }))}
            />
          </Row>
          <Row label="Quick actions" description="Show starter prompts in empty threads">
            <Toggle
              label="Quick actions"
              checked={showQuickActions}
              onChange={setShowQuickActions}
            />
          </Row>
        </Section>
      </div>
    </div>
  );
};
