import { api } from '../api';

/** True when `git push` was refused because the remote has commits the local branch lacks. */
export const isPushRejected = (msg: string) =>
  /rejected|fetch first|non-fast-forward|failed to push some refs|tip of your current branch is behind/i.test(
    msg
  );

export type PullMode = 'merge' | 'rebase';

/** Pulls the remote changes, then pushes again. Throws with git's message on failure. */
export async function pullThenPush(mode: PullMode): Promise<void> {
  const pull = await api.gitPull(mode);
  if (!pull.ok) throw new Error((pull.stderr || pull.stdout || 'Pull failed').trim());
  const push = await api.gitPush(false);
  if (!push.ok) throw new Error((push.stderr || push.stdout || 'Push failed').trim());
}
