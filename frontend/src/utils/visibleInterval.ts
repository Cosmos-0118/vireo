/**
 * `setInterval` that does nothing while the window is hidden/minimized and fires once
 * immediately when it becomes visible again. Returns the cleanup function.
 */
export function visibleInterval(fn: () => void, ms: number): () => void {
  const id = window.setInterval(() => {
    if (!document.hidden) fn();
  }, ms);
  const onVisible = () => {
    if (!document.hidden) fn();
  };
  document.addEventListener('visibilitychange', onVisible);
  return () => {
    window.clearInterval(id);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
