/**
 * Git operations that rewrite the working tree (pull, checkout, stash, reset, ...) fire the same
 * watcher events as an agent editing files. They are tracked here so those events are not
 * announced as agent edits.
 */

/** Covers the watcher's debounce/batching plus the frontend flush after the op returns. */
const QUIET_MS = 2000;

let active = 0;
let quietUntil = 0;

export function gitWriteActive(): boolean {
  return active > 0 || Date.now() < quietUntil;
}

export async function trackGitWrite<T>(op: () => Promise<T>): Promise<T> {
  active++;
  try {
    return await op();
  } finally {
    active--;
    quietUntil = Date.now() + QUIET_MS;
  }
}
