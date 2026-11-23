import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Lock, ShieldAlert, ShieldCheck } from 'lucide-react';
import { api } from './api';
import type { AgentConfig, CliToolStatus, LocalModelsInfo, PermissionMode } from './types';

export type Provider = AgentConfig['provider'];

export const DEFAULT_CONFIG: AgentConfig = {
  provider: 'claude-cli',
  baseUrl: '',
  apiKey: '',
  model: 'default',
  autoCheckpoint: true,
};

/** Starting model per provider. CLIs keep their own configured model; API ids come from the live list. */
export const PROVIDER_PRESETS: Record<Provider, { model: string; baseUrl: string }> = {
  openai: { model: '', baseUrl: 'https://api.openai.com/v1' },
  anthropic: { model: 'claude-sonnet-5-5', baseUrl: 'https://api.anthropic.com' },
  gemini: { model: '', baseUrl: '' },
  ollama: { model: '', baseUrl: 'http://127.0.0.1:11434/v1' },
  'claude-cli': { model: 'default', baseUrl: '' },
  'codex-cli': { model: 'default', baseUrl: '' },
  'copilot-cli': { model: 'default', baseUrl: '' },
};

/**
 * Static options only where the CLI itself defines them (Claude Code aliases track the latest
 * model). Everything else is discovered at runtime from the CLI (Codex), Ollama, or the provider
 * API; otherwise just "default" + a custom id.
 */
const STATIC_MODELS: Partial<Record<Provider, string[]>> = {
  'claude-cli': ['default', 'sonnet', 'opus', 'haiku'],
};

export const PROVIDER_LABEL: Record<Provider, string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
  ollama: 'Ollama',
  'claude-cli': 'Claude Code',
  'codex-cli': 'Codex',
  'copilot-cli': 'GitHub Copilot',
};

/** Provider ids whose API keys are kept in the OS credential store. */
export const SECRET_NAMES = Object.keys(PROVIDER_LABEL);

/** Providers that run an installed CLI with its own login instead of an API key. */
export const CLI_PROVIDER_TOOL: Partial<Record<Provider, string>> = {
  'claude-cli': 'claude',
  'codex-cli': 'codex',
  'copilot-cli': 'copilot',
};

export const CLI_PROVIDERS: Provider[] = ['claude-cli', 'codex-cli', 'copilot-cli'];
export const API_PROVIDERS: Provider[] = ['anthropic', 'openai', 'gemini'];

export const PERM_MODES: Record<
  PermissionMode,
  { label: string; icon: typeof Lock; claudeNote: string; codexNote: string; cliNote: string }
> = {
  ask: {
    label: 'Ask',
    icon: Lock,
    claudeNote: 'Asks before every edit and shell command.',
    codexNote: 'Asks before every edit and command; the workspace sandbox stays on.',
    cliNote: 'Asks before edits and commands where the CLI can prompt; otherwise read-only.',
  },
  auto_edit: {
    label: 'Auto-edit',
    icon: ShieldCheck,
    claudeNote: 'Edits in the workspace are automatic; anything else asks.',
    codexNote: 'Edits and commands run in the workspace sandbox; Codex asks when it needs more.',
    cliNote: 'Workspace edits are auto-accepted using the CLI’s safety mode.',
  },
  full_access: {
    label: 'Full access',
    icon: ShieldAlert,
    claudeNote: 'Never asks. Can change any file and run any command.',
    codexNote: 'No sandbox, no approvals. Full access to this machine.',
    cliNote: 'Auto-approves tools and commands without prompts.',
  },
};

const LEGACY_KEYS_STORE = 'vireo-agent-keys';
const CONFIG_KEY = 'vireo-agent-config';
const QUICK_ACTIONS_KEY = 'vireo-show-quick-actions';

interface AgentSettingsValue {
  config: AgentConfig;
  setConfig: React.Dispatch<React.SetStateAction<AgentConfig>>;
  apiKeys: Record<string, string>;
  setProvider: (p: Provider) => void;
  cliTools: CliToolStatus[];
  localModels: LocalModelsInfo | null;
  ollamaModels: string[];
  /** Live ids from the active API provider; null until loaded. */
  apiModels: string[] | null;
  apiModelsError: string | null;
  /** Models selectable for the active provider. */
  modelOptions: string[];
  /** Re-detect installed CLIs and models. */
  refresh: () => void;
  showQuickActions: boolean;
  setShowQuickActions: (v: boolean) => void;
}

const Ctx = createContext<AgentSettingsValue | null>(null);

export function useAgentSettings(): AgentSettingsValue {
  const v = useContext(Ctx);
  if (!v) throw new Error('useAgentSettings must be used inside <AgentSettingsProvider>');
  return v;
}

/** Single owner of agent config, API keys and detected engines, shared by the panel and Settings tab. */
export const AgentSettingsProvider: React.FC<{
  onToast: (msg: string, isError?: boolean) => void;
  children: React.ReactNode;
}> = ({ onToast, children }) => {
  const [config, setConfig] = useState<AgentConfig>(() => {
    try {
      const saved = localStorage.getItem(CONFIG_KEY);
      if (saved) {
        const parsed = JSON.parse(saved);
        // A provider that was removed in an update falls back to the default engine.
        if (parsed.provider in PROVIDER_PRESETS) return { ...DEFAULT_CONFIG, ...parsed };
      }
    } catch {
      // ignore
    }
    return DEFAULT_CONFIG;
  });
  const [secretsReady, setSecretsReady] = useState(false);
  const savedSecretsRef = useRef<Record<string, string>>({});
  const [apiKeys, setApiKeys] = useState<Record<string, string>>(() => {
    try {
      // Legacy clear-text store; migrated to the OS credential store on mount, then removed.
      return JSON.parse(localStorage.getItem(LEGACY_KEYS_STORE) || '{}');
    } catch {
      return {};
    }
  });
  const [showQuickActions, setShowQuickActionsState] = useState(() => {
    try {
      return localStorage.getItem(QUICK_ACTIONS_KEY) === '1';
    } catch {
      return false;
    }
  });
  const setShowQuickActions = useCallback((v: boolean) => {
    setShowQuickActionsState(v);
    try {
      localStorage.setItem(QUICK_ACTIONS_KEY, v ? '1' : '0');
    } catch {
      // ignore
    }
  }, []);

  const [cliTools, setCliTools] = useState<CliToolStatus[]>([]);
  const [localModels, setLocalModels] = useState<LocalModelsInfo | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const refresh = useCallback(() => setRefreshTick((t) => t + 1), []);

  useEffect(() => {
    let cancelled = false;
    api
      .cliTools()
      .then((t) => !cancelled && setCliTools(t))
      .catch(() => {});
    api
      .agentModels(config.provider === 'ollama' ? config.baseUrl : undefined)
      .then((m) => !cancelled && setLocalModels(m))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshTick, config.provider]);

  // Live model ids for API providers (debounced so typing a key doesn't spam the provider).
  const [apiModels, setApiModels] = useState<{ key: string; list: string[]; error: string | null } | null>(null);
  const isApi = API_PROVIDERS.includes(config.provider);
  const apiModelsKey = `${config.provider}|${config.baseUrl}|${config.apiKey}|${refreshTick}`;
  useEffect(() => {
    if (!isApi) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .apiModels(config.provider, config.baseUrl, config.apiKey)
        .then((list) => !cancelled && setApiModels({ key: apiModelsKey, list, error: null }))
        .catch(
          (e: unknown) =>
            !cancelled &&
            setApiModels({ key: apiModelsKey, list: [], error: e instanceof Error ? e.message : String(e) })
        );
    }, 500);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apiModelsKey, isApi]);
  const apiModelsCurrent = isApi && apiModels?.key === apiModelsKey ? apiModels : null;

  const ollamaModels = useMemo(
    () => (localModels?.ollama.running ? localModels.ollama.models : []),
    [localModels]
  );

  const modelOptions = useMemo(() => {
    const p = config.provider;
    let list: string[];
    if (p === 'ollama') list = ollamaModels;
    else if (p === 'codex-cli') list = ['default', ...(localModels?.codex ?? [])];
    else if (isApi) list = apiModelsCurrent?.list ?? [];
    else list = STATIC_MODELS[p] ?? ['default'];
    // Always keep the active model selectable, even when it is a custom id.
    return config.model && !list.includes(config.model) ? [config.model, ...list] : list;
  }, [config.provider, config.model, isApi, ollamaModels, localModels, apiModelsCurrent]);

  const setProvider = useCallback(
    (p: Provider) =>
      setConfig((prev) => ({
        ...prev,
        provider: p,
        model: p === 'ollama' && ollamaModels.length ? ollamaModels[0] : PROVIDER_PRESETS[p].model,
        baseUrl: PROVIDER_PRESETS[p].baseUrl,
        apiKey: apiKeys[p] ?? '',
      })),
    [apiKeys, ollamaModels]
  );

  // Once a live list arrives and no model is chosen yet, pick the first one.
  useEffect(() => {
    if (apiModelsCurrent && apiModelsCurrent.list.length && !config.model) {
      setConfig((c) => (c.model ? c : { ...c, model: apiModelsCurrent.list[0] }));
    }
  }, [apiModelsCurrent, config.model]);

  // Persist everything except the API key, which goes to the OS credential store instead.
  useEffect(() => {
    try {
      localStorage.setItem(CONFIG_KEY, JSON.stringify({ ...config, apiKey: '' }));
    } catch {
      // ignore
    }
  }, [config]);

  useEffect(() => {
    setApiKeys((prev) =>
      prev[config.provider] === config.apiKey ? prev : { ...prev, [config.provider]: config.apiKey }
    );
  }, [config.provider, config.apiKey]);

  // Load keys from the OS credential store; keys a previous version left in localStorage are
  // moved there once and then erased from the browser.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      let stored: Record<string, string> = {};
      try {
        stored = await api.getSecrets(SECRET_NAMES);
      } catch {
        // credential store unavailable: fall back to whatever is in memory
      }
      if (cancelled) return;
      const merged: Record<string, string> = { ...stored };
      for (const [k, v] of Object.entries(apiKeys)) if (v.trim()) merged[k] = v;
      const saved = { ...stored };
      let migrated = true;
      for (const name of SECRET_NAMES) {
        if ((merged[name] ?? '') === (stored[name] ?? '')) continue;
        try {
          await api.putSecret(name, merged[name]);
          saved[name] = merged[name];
        } catch {
          migrated = false;
        }
      }
      if (cancelled) return;
      if (migrated) {
        try {
          localStorage.removeItem(LEGACY_KEYS_STORE);
        } catch {
          // ignore
        }
      }
      savedSecretsRef.current = saved;
      setApiKeys(merged);
      setConfig((c) => (c.apiKey ? c : { ...c, apiKey: merged[c.provider] ?? '' }));
      setSecretsReady(true);
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Write changed keys back to the credential store (debounced while typing).
  useEffect(() => {
    if (!secretsReady) return;
    const timer = setTimeout(() => {
      for (const name of SECRET_NAMES) {
        const value = apiKeys[name] ?? '';
        if (value === (savedSecretsRef.current[name] ?? '')) continue;
        api
          .putSecret(name, value)
          .then(() => {
            savedSecretsRef.current[name] = value;
          })
          .catch(() => onToast('Could not save API key to the OS credential store', true));
      }
    }, 600);
    return () => clearTimeout(timer);
  }, [apiKeys, secretsReady, onToast]);

  const value = useMemo<AgentSettingsValue>(
    () => ({
      config,
      setConfig,
      apiKeys,
      setProvider,
      cliTools,
      localModels,
      ollamaModels,
      apiModels: apiModelsCurrent ? apiModelsCurrent.list : null,
      apiModelsError: apiModelsCurrent?.error ?? null,
      modelOptions,
      refresh,
      showQuickActions,
      setShowQuickActions,
    }),
    [
      config,
      apiKeys,
      setProvider,
      cliTools,
      localModels,
      ollamaModels,
      apiModelsCurrent,
      modelOptions,
      refresh,
      showQuickActions,
      setShowQuickActions,
    ]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
};
