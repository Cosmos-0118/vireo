import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import '../index.css';
import { AgentEditsStrip } from '../components/AgentEditsStrip';
const paths = ['frontend/src/api.ts','frontend/src/types.ts','backend/src/main.rs','backend/src/agent.rs','backend/src/cli_agent.rs','backend/src/pty.rs','frontend/src/components/AgentPanel.tsx','frontend/src/App.tsx','backend/src/git/mod.rs','backend/src/fs/mod.rs','frontend/src/index.css','README.md'];
const n = Number(new URLSearchParams(location.search).get('n') || 12);
const edits = paths.slice(0, n).reverse().map((p, i) => ({ path: p, adds: (i * 37) % 700 + 1, dels: i % 5, at: i }));
function App() {
  const [active, setActive] = useState<string | null>(paths[0]);
  return <div style={{ height: '100vh', display: 'flex', flexDirection: 'column', justifyContent: 'flex-end', background: 'var(--bg-app, #111)' }}>
    <div id="active" style={{ color: 'white', padding: 8 }}>active: {active}</div>
    <div className="shrink-0 flex items-center gap-2 h-9 px-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)] text-xs">
      <AgentEditsStrip edits={edits} activePath={active} tabPulse={{}} onFocus={setActive} />
      <div className="flex items-center gap-1.5 shrink-0"><button className="h-6 px-2 rounded-md bg-[var(--accent)] text-[var(--accent-fg)]">Keep all</button></div>
    </div>
  </div>;
}
createRoot(document.getElementById('root')!).render(<App />);
