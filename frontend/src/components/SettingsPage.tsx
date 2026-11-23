import React, { useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  Cloud,
  Copy,
  Cpu,
  Eye,
  EyeOff,
  KeyRound,
  RefreshCw,
  RotateCcw,
  Shield,
  SlidersHorizontal,
  Sparkles,
  Terminal,
  X,
  Zap,
} from 'lucide-react';
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
  'w-full h-9 px-3 rounded-lg bg-[var(--bg-input)] border border-[var(--border)] text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none transition-[border-color,box-shadow] hover:border-[color-mix(in_srgb,var(--text-dim)_65%,var(--border))] focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_18%,transparent)] text-ellipsis';

const Section: React.FC<{
  title: string;
  hint?: string;
  action?: React.ReactNode;
  bare?: boolean;
  children: React.ReactNode;
}> = ({ title, hint, action, bare, children }) => (
  <section className="mb-8 last:mb-0">
    <div className="flex items-end justify-between gap-4 mb-3">
      <div className="min-w-0">
        <h2 className="text-[11.5px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">{title}</h2>
        {hint && <p className="mt-0.5 text-[12px] text-[var(--text-dim)]">{hint}</p>}
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
    {bare ? (
      children
    ) : (
      <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-panel)] divide-y divide-[var(--border)]/80 shadow-[0_1px_2px_rgba(0,0,0,0.12)]">
        {children}
      </div>
    )}
  </section>
);

const Row: React.FC<{
  label: string;
  description?: React.ReactNode;
  icon?: React.ReactNode;
  align?: 'start' | 'center';
  children: React.ReactNode;
}> = ({ label, description, icon, align = 'center', children }) => (
  <div
    className={`flex flex-col sm:flex-row ${
      align === 'start' ? 'sm:items-start' : 'sm:items-center'
    } justify-between gap-3 sm:gap-6 px-4 py-3.5`}
  >
    <div className="flex items-start gap-2.5 min-w-0 sm:max-w-[44%]">
      {icon && (
        <span className="mt-0.5 w-7 h-7 rounded-lg bg-[var(--bg-elevated)] border border-[var(--border)]/80 text-[var(--text-muted)] flex items-center justify-center shrink-0">
          {icon}
        </span>
      )}
      <div className="min-w-0">
        <div className="text-[13px] font-medium text-[var(--text-primary)] leading-snug">{label}</div>
        {description && (
          <div className="mt-0.5 text-[12px] leading-snug text-[var(--text-dim)]">{description}</div>
        )}
      </div>
    </div>
    <div className="w-full sm:w-[54%] min-w-0 flex sm:justify-end">{children}</div>
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
    className={`relative w-9 h-5 rounded-full shrink-0 cursor-pointer transition-colors outline-none focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_25%,transparent)] ${
      checked
        ? 'bg-[var(--accent)]'
        : 'bg-[var(--bg-elevated)] border border-[var(--border)] hover:border-[var(--text-dim)]'
    }`}
  >
    <span
      className={`absolute top-1/2 -translate-y-1/2 left-[2px] w-4 h-4 rounded-full bg-white shadow-[0_1px_3px_rgba(0,0,0,0.35)] transition-transform duration-150 ${
        checked ? 'translate-x-[16px]' : 'translate-x-0'
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

  // Re-detect installed engines every time the page is opened.
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Hide revealed API key whenever the active provider changes.
  useEffect(() => {
    setShowKey(false);
    setCopiedHint(false);
  }, [config.provider]);

  const triggerRefresh = () => {
    refresh();
    setRefreshing(true);
    window.setTimeout(() => setRefreshing(false), 650);
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
        hint: 'Uses CLI login · no API key required',
        icon: <Terminal size={14} />,
        group: 'CLI agents',
        dot: installed === true ? 'ok' : installed === false ? 'warn' : 'muted',
        badge: installed === true ? 'Installed' : installed === false ? 'Not found' : undefined,
        badgeTone: installed === true ? 'ok' : 'warn',
      };
    });

    const apiMeta: Record<'anthropic' | 'openai' | 'gemini', { label: string; hint: string; icon: React.ReactNode }> = {
      anthropic: {
        label: 'Anthropic API',
        hint: 'Direct Claude models via API key',
        icon: <Sparkles size={14} />,
      },
      openai: {
        label: 'OpenAI / Compatible API',
        hint: 'OpenAI, OpenRouter, DeepSeek, or LM Studio',
        icon: <Cloud size={14} />,
      },
      gemini: {
        label: 'Google Gemini API',
        hint: 'Direct Gemini models via API key',
        icon: <Sparkles size={14} />,
      },
    };

    const apiOpts: SelectOption[] = (['anthropic', 'openai', 'gemini'] as const).map((p) => {
      const hasKey = !!(apiKeys[p] ?? '').trim();
      const meta = apiMeta[p];
      return {
        value: p,
        label: meta.label,
        hint: meta.hint,
        icon: meta.icon,
        group: 'API & local',
        dot: hasKey ? 'ok' : 'muted',
        badge: hasKey ? 'Key saved' : undefined,
        badgeTone: 'ok',
      };
    });

    const ollamaRunning = localModels?.ollama.running;
    const ollamaCount = localModels?.ollama.models.length ?? 0;
    const ollamaOpt: SelectOption = {
      value: 'ollama',
      label: 'Ollama (Local)',
      hint: 'Run local models on your machine',
      icon: <Cpu size={14} />,
      group: 'API & local',
      dot: ollamaRunning === true ? 'ok' : ollamaRunning === false ? 'warn' : 'muted',
      badge:
        ollamaRunning === true
          ? `${ollamaCount} model${ollamaCount === 1 ? '' : 's'}`
          : ollamaRunning === false
            ? 'Offline'
            : undefined,
      badgeTone: ollamaRunning === true ? 'ok' : 'muted',
    };

    return [...cliOpts, ...apiOpts, ollamaOpt];
  }, [cliTools, apiKeys, localModels]);

  const modelSuggestions = useMemo<SelectOption[]>(
    () =>
      modelOptions.map((m) =>
        m === 'default'
          ? {
              value: m,
              label: m,
              hint: isCli ? 'Uses the CLI’s configured default model' : undefined,
              badge: 'Default',
              badgeTone: 'muted',
            }
          : { value: m, label: m }
      ),
    [modelOptions, isCli]
  );

  let status: { ok: boolean; title: string; detail?: string; installHint?: string } | null = null;
  if (isCli) {
    status = tool
      ? tool.installed
        ? { ok: true, title: `${PROVIDER_LABEL[config.provider]} CLI detected`, detail: tool.path || undefined }
        : {
            ok: false,
            title: `${PROVIDER_LABEL[config.provider]} CLI not found on PATH`,
            installHint: tool.install_hint || undefined,
          }
      : null;
  } else if (isOllama) {
    status = localModels
      ? localModels.ollama.running
        ? {
            ok: true,
            title: 'Ollama is running',
            detail: `${localModels.ollama.models.length} local model${localModels.ollama.models.length === 1 ? '' : 's'} available`,
          }
        : { ok: false, title: 'Ollama is not reachable at this address.' }
      : null;
  } else {
    status = config.apiKey.trim()
      ? apiModelsError
        ? { ok: false, title: 'Could not reach provider', detail: apiModelsError }
        : {
            ok: true,
            title: 'API key stored in OS credential manager',
            detail: modelOptions.length > 0 ? `${modelOptions.length} models loaded` : undefined,
          }
      : { ok: false, title: 'Add an API key below to connect and list available models.' };
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
      <div className="mx-auto w-full max-w-[760px] px-6 py-7">
        {/* Page Header */}
        <header className="flex flex-wrap items-start justify-between gap-4 pb-6 mb-7 border-b border-[var(--border)]/80">
          <div className="flex items-start gap-3 min-w-0">
            <div className="w-9 h-9 rounded-xl bg-[color-mix(in_srgb,var(--accent)_14%,var(--bg-elevated))] border border-[color-mix(in_srgb,var(--accent)_28%,var(--border))] text-[var(--accent)] flex items-center justify-center shrink-0">
              <SlidersHorizontal size={17} />
            </div>
            <div className="min-w-0">
              <h1 className="text-[18px] font-semibold tracking-tight text-[var(--text-primary)] leading-tight">
                Agent Settings
              </h1>
              <p className="mt-1 text-[12.5px] text-[var(--text-dim)]">
                Engine, model selection, tool permissions, and workspace behavior for the agent panel.
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2 shrink-0">
            {status && (
              <div
                className={`inline-flex items-center gap-1.5 h-7 px-2.5 rounded-full border text-[11.5px] font-medium ${
                  status.ok
                    ? 'bg-[var(--added-bg)] border-[color-mix(in_srgb,var(--added-fg)_26%,transparent)] text-[var(--added-fg)]'
                    : 'bg-[color-mix(in_srgb,var(--modified-fg)_12%,transparent)] border-[color-mix(in_srgb,var(--modified-fg)_28%,transparent)] text-[var(--modified-fg)]'
                }`}
              >
                <span
                  className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                    status.ok ? 'bg-[var(--added-fg)]' : 'bg-[var(--modified-fg)]'
                  }`}
                />
                <span>
                  {PROVIDER_LABEL[config.provider]} · {status.ok ? 'Ready' : 'Needs setup'}
                </span>
              </div>
            )}
            <button
              type="button"
              onClick={triggerRefresh}
              title="Re-detect installed CLI tools and refresh models"
              className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg border border-[var(--border)] bg-[var(--bg-panel)] text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
            >
              <RefreshCw size={12} className={refreshing ? 'animate-spin text-[var(--accent)]' : ''} />
              Refresh
            </button>
          </div>
        </header>

        {/* Engine & Model */}
        <Section
          title="Engine & Model"
          hint="Run an installed CLI agent with its existing login, or connect directly to an API or local server."
        >
          <Row
            label="Provider"
            description={
              isCli
                ? 'Uses the CLI’s own authentication — no API key needed.'
                : isOllama
                  ? 'Connects to a locally running Ollama instance.'
                  : 'Connects directly with your provider API key.'
            }
          >
            <Select
              ariaLabel="Provider"
              value={config.provider}
              options={providerOptions}
              onChange={(v) => setProvider(v as Provider)}
            />
          </Row>

          {status && (
            <div className="px-4 py-2.5 bg-[var(--bg-base)]/40">
              <div
                className={`flex items-start gap-2.5 px-3 py-2 rounded-lg border text-[12px] ${
                  status.ok
                    ? 'bg-[var(--added-bg)]/70 border-[color-mix(in_srgb,var(--added-fg)_22%,transparent)] text-[var(--added-fg)]'
                    : 'bg-[color-mix(in_srgb,var(--modified-fg)_9%,transparent)] border-[color-mix(in_srgb,var(--modified-fg)_24%,transparent)] text-[var(--modified-fg)]'
                }`}
              >
                {status.ok ? (
                  <CheckCircle2 size={14} className="mt-0.5 shrink-0" />
                ) : (
                  <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                )}
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="font-medium">{status.title}</span>
                    {status.detail && (
                      <span
                        className="font-mono text-[11px] px-1.5 py-px rounded bg-[var(--bg-base)]/60 text-[var(--text-muted)] border border-[var(--border)]/70 truncate max-w-full"
                        title={status.detail}
                      >
                        {status.detail}
                      </span>
                    )}
                  </div>
                  {status.installHint && (
                    <div className="mt-1.5 flex items-center gap-2 text-[11.5px] text-[var(--text-muted)]">
                      <code className="px-2 py-0.5 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[11px] text-[var(--text-primary)] truncate">
                        {status.installHint}
                      </code>
                      <button
                        type="button"
                        onClick={() => copyInstallHint(status!.installHint!)}
                        className="shrink-0 inline-flex items-center gap-1 h-6 px-2 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                        title="Copy install command"
                      >
                        {copiedHint ? <Check size={11} className="text-[var(--added-fg)]" /> : <Copy size={11} />}
                        {copiedHint ? 'Copied' : 'Copy'}
                      </button>
                    </div>
                  )}
                </div>
              </div>
            </div>
          )}

          <Row
            label="Model"
            align={modelOptions.length > 0 && modelOptions.length <= 6 ? 'start' : 'center'}
            description={
              isCli
                ? '“default” keeps the CLI’s configured model. Pick a preset or type any model ID.'
                : isApi
                  ? modelOptions.length > 0
                    ? `${modelOptions.length} models loaded live from ${PROVIDER_LABEL[config.provider]}.`
                    : 'Loaded live once a valid API key is set, or type any model ID.'
                  : 'Select a pulled local model or type a model tag.'
            }
          >
            <div className="w-full min-w-0 space-y-2">
              <Combobox
                ariaLabel="Model"
                value={config.model}
                suggestions={modelSuggestions}
                onChange={(v) => setConfig((prev) => ({ ...prev, model: v }))}
                placeholder={isApi ? 'Pick a model or type a custom ID…' : 'Model ID'}
                trailing={
                  <button
                    type="button"
                    onClick={triggerRefresh}
                    title="Refresh available models"
                    className="h-9 w-9 shrink-0 flex items-center justify-center rounded-lg border border-[var(--border)] bg-[var(--bg-input)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] hover:border-[color-mix(in_srgb,var(--text-dim)_65%,var(--border))] cursor-pointer transition-colors"
                  >
                    <RefreshCw size={13} className={refreshing ? 'animate-spin text-[var(--accent)]' : ''} />
                  </button>
                }
              />

              {modelOptions.length > 0 && modelOptions.length <= 6 && (
                <div className="flex flex-wrap gap-1.5">
                  {modelOptions.map((m) => {
                    const selected = config.model === m;
                    return (
                      <button
                        key={m}
                        type="button"
                        onClick={() => setConfig((prev) => ({ ...prev, model: m }))}
                        className={`inline-flex items-center gap-1 h-6 max-w-full px-2 rounded-md border text-[11.5px] font-mono cursor-pointer transition-colors ${
                          selected
                            ? 'border-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-[var(--text-primary)]'
                            : 'border-[var(--border)] bg-[var(--bg-input)]/70 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                        }`}
                      >
                        {selected && <Check size={10} className="text-[var(--accent)] shrink-0" />}
                        <span className="truncate">{m}</span>
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </Row>

          {(isApi || isOllama) && config.provider !== 'gemini' && (
            <Row
              label="Base URL"
              description={
                isOllama
                  ? 'Address where your Ollama server is listening.'
                  : 'Optional. Point at OpenRouter, DeepSeek, LM Studio, or any compatible endpoint.'
              }
            >
              <div className="relative w-full">
                <input
                  value={config.baseUrl}
                  onChange={(e) => setConfig((prev) => ({ ...prev, baseUrl: e.target.value }))}
                  placeholder={isOllama ? 'http://127.0.0.1:11434/v1' : 'https://api.openai.com/v1'}
                  spellCheck={false}
                  className={`${fieldCls} font-mono text-[12px] ${baseUrlModified ? 'pr-16' : ''}`}
                />
                {baseUrlModified && (
                  <button
                    type="button"
                    onClick={() => setConfig((prev) => ({ ...prev, baseUrl: defaultBaseUrl }))}
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 h-6 px-2 rounded-md text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    title="Reset to default endpoint"
                  >
                    Reset
                  </button>
                )}
              </div>
            </Row>
          )}

          {isApi && (
            <Row
              label="API key"
              description="Stored securely in your OS credential manager, never in browser storage."
            >
              <div className="relative w-full">
                <KeyRound
                  size={13}
                  className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-dim)]"
                />
                <input
                  type={showKey ? 'text' : 'password'}
                  value={config.apiKey}
                  onChange={(e) => setConfig((prev) => ({ ...prev, apiKey: e.target.value }))}
                  placeholder="sk-…"
                  autoComplete="off"
                  spellCheck={false}
                  className={`${fieldCls} font-mono text-[12px] pl-8 ${config.apiKey ? 'pr-16' : 'pr-9'}`}
                />
                <div className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center gap-0.5">
                  {config.apiKey && (
                    <button
                      type="button"
                      onClick={() => setConfig((prev) => ({ ...prev, apiKey: '' }))}
                      className="w-6 h-6 rounded-md flex items-center justify-center text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                      title="Clear API key"
                      aria-label="Clear API key"
                    >
                      <X size={13} />
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => setShowKey((v) => !v)}
                    className="w-6 h-6 rounded-md flex items-center justify-center text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    title={showKey ? 'Hide API key' : 'Reveal API key'}
                    aria-label={showKey ? 'Hide API key' : 'Reveal API key'}
                  >
                    {showKey ? <EyeOff size={13} /> : <Eye size={13} />}
                  </button>
                </div>
              </div>
            </Row>
          )}
        </Section>

        {/* Permissions (CLI agents) */}
        {isCli && (
          <Section
            title="Permissions"
            hint="Control what the CLI agent may run or edit without asking for approval."
            bare
          >
            <div role="radiogroup" aria-label="Agent permissions" className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
              {(Object.keys(PERM_MODES) as PermissionMode[]).map((mode) => {
                const m = PERM_MODES[mode];
                const Icon = m.icon;
                const active = permMode === mode;
                const risky = mode === 'full_access';
                return (
                  <button
                    key={mode}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    onClick={() => setConfig((prev) => ({ ...prev, permissionMode: mode }))}
                    className={`group flex flex-col p-3.5 rounded-xl border text-left cursor-pointer transition-all outline-none focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_22%,transparent)] ${
                      active
                        ? risky
                          ? 'border-[var(--modified-fg)] bg-[color-mix(in_srgb,var(--modified-fg)_9%,var(--bg-panel))]'
                          : 'border-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_9%,var(--bg-panel))]'
                        : 'border-[var(--border)] bg-[var(--bg-panel)] hover:bg-[var(--bg-hover)]/70 hover:border-[color-mix(in_srgb,var(--text-dim)_60%,var(--border))]'
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2 mb-2.5">
                      <span
                        className={`w-7 h-7 rounded-lg flex items-center justify-center transition-colors ${
                          active
                            ? risky
                              ? 'bg-[color-mix(in_srgb,var(--modified-fg)_18%,transparent)] text-[var(--modified-fg)]'
                              : 'bg-[color-mix(in_srgb,var(--accent)_18%,transparent)] text-[var(--accent)]'
                            : 'bg-[var(--bg-elevated)] text-[var(--text-muted)] group-hover:text-[var(--text-primary)]'
                        }`}
                      >
                        <Icon size={14} />
                      </span>
                      <span
                        className={`w-4 h-4 rounded-full border flex items-center justify-center transition-colors ${
                          active
                            ? risky
                              ? 'border-[var(--modified-fg)]'
                              : 'border-[var(--accent)]'
                            : 'border-[var(--text-dim)]/60'
                        }`}
                      >
                        {active && (
                          <span
                            className={`w-2 h-2 rounded-full ${
                              risky ? 'bg-[var(--modified-fg)]' : 'bg-[var(--accent)]'
                            }`}
                          />
                        )}
                      </span>
                    </div>
                    <div className="flex items-center gap-1.5">
                      <span className="text-[13px] font-medium text-[var(--text-primary)]">{m.label}</span>
                      {risky && (
                        <span className="text-[10px] font-medium px-1.5 py-px rounded-full border border-[color-mix(in_srgb,var(--modified-fg)_30%,transparent)] bg-[color-mix(in_srgb,var(--modified-fg)_10%,transparent)] text-[var(--modified-fg)]">
                          Elevated
                        </span>
                      )}
                    </div>
                    <p className="mt-1 text-[11.5px] leading-relaxed text-[var(--text-dim)]">{permNote(mode)}</p>
                  </button>
                );
              })}
            </div>
          </Section>
        )}

        {/* Workspace & Panel Preferences */}
        <Section title="Preferences" hint="Safety checkpoints and agent panel defaults.">
          <Row
            icon={<RotateCcw size={14} />}
            label="Checkpoint before agent edits"
            description="Snapshots the workspace before each turn so changes can be undone cleanly."
          >
            <Toggle
              label="Checkpoint before agent edits"
              checked={config.autoCheckpoint}
              onChange={(v) => setConfig((prev) => ({ ...prev, autoCheckpoint: v }))}
            />
          </Row>
          <Row
            icon={<Zap size={14} />}
            label="Show quick actions"
            description="Display suggested starter prompts when opening an empty agent thread."
          >
            <Toggle label="Show quick actions" checked={showQuickActions} onChange={setShowQuickActions} />
          </Row>
        </Section>

        <footer className="mt-6 flex items-center gap-1.5 text-[11.5px] text-[var(--text-dim)]">
          <Shield size={12} className="shrink-0" />
          <span>Changes save automatically and apply immediately to the agent panel.</span>
        </footer>
      </div>
    </div>
  );
};
