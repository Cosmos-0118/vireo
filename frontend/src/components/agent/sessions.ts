import { CLI_PROVIDER_TOOL, type Provider } from '../../agentSettings';

/** CLI providers run as one persistent agent process per chat thread (the API providers do not). */
export const supportsSessions = (provider: Provider): boolean => provider in CLI_PROVIDER_TOOL;

/** An agent conversation belongs to one (thread, provider) pair: switching engines starts a new one. */
export const sessionKey = (threadId: string, provider: Provider): string => `${threadId}:${provider}`;
