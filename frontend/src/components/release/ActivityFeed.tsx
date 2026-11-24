import type { ReleaseActivity } from '../../types';

export function activityTitle(e: ReleaseActivity): string {
  const tag = e.tag ?? e.version ?? '';
  switch (e.action) {
    case 'preflight':
      return `Ran readiness checks${tag ? ` for ${tag}` : ''}`;
    case 'notes':
      return `Wrote release notes${tag ? ` for ${tag}` : ''}`;
    case 'bump':
      return `Bumped versions${e.version ? ` to ${e.version}` : ''}`;
    case 'verify':
      return `Verified ${tag}`;
    case 'publish':
      if (e.phase === 'running') return `Releasing ${tag}…`;
      if (e.dry_run) return `Planned release ${tag}`;
      return e.ok ? `Released ${tag}` : `Release ${tag} stopped`;
  }
}
