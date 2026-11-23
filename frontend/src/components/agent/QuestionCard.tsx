import React, { useState } from 'react';
import { MessageCircleQuestion } from 'lucide-react';
import type { AgentQuestion, QuestionRequest } from '../../types';

interface QuestionCardProps {
  request: QuestionRequest;
  onSubmit: (request: QuestionRequest, answers: Record<string, string[]>) => void;
  onDismiss: (request: QuestionRequest) => void;
}

/** In-chat multiple-choice questions from the agent (e.g. Claude's AskUserQuestion). */
export const QuestionCard: React.FC<QuestionCardProps> = ({ request, onSubmit, onDismiss }) => {
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});

  const toggle = (q: AgentQuestion, label: string) =>
    setPicked((prev) => {
      const cur = prev[q.id] ?? [];
      const next = q.multi_select
        ? cur.includes(label)
          ? cur.filter((l) => l !== label)
          : [...cur, label]
        : [label];
      return { ...prev, [q.id]: next };
    });

  /** Chosen options plus the typed answer, which replaces the choice for single-choice questions. */
  const answerFor = (q: AgentQuestion): string[] => {
    const typed = (other[q.id] ?? '').trim();
    const chosen = picked[q.id] ?? [];
    if (!typed) return chosen;
    return q.multi_select ? [...chosen, typed] : [typed];
  };
  const complete = request.questions.every((q) => answerFor(q).length > 0);

  const submit = () => {
    const answers: Record<string, string[]> = {};
    for (const q of request.questions) answers[q.id] = answerFor(q);
    onSubmit(request, answers);
  };

  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] px-3 py-2 space-y-2.5">
      <div className="flex items-center gap-1.5 text-[11px] text-[var(--text-muted)]">
        <MessageCircleQuestion size={11} className="shrink-0" />
        The agent has a question
      </div>
      {request.questions.map((q) => {
        const typed = (other[q.id] ?? '').trim() !== '' && !q.multi_select;
        return (
          <div key={q.id} className="space-y-1.5">
            {q.header && (
              <span className="inline-block px-1.5 rounded bg-[var(--bg-hover)] text-[10.5px] text-[var(--text-muted)]">
                {q.header}
              </span>
            )}
            <div className="text-[12.5px] text-[var(--text-primary)]">{q.question}</div>
            <div className="space-y-1">
              {q.options.map((o) => {
                const on = !typed && (picked[q.id] ?? []).includes(o.label);
                return (
                  <button
                    key={o.label}
                    onClick={() => {
                      if (!q.multi_select) setOther((prev) => ({ ...prev, [q.id]: '' }));
                      toggle(q, o.label);
                    }}
                    className={`w-full text-left rounded-md border px-2 py-1 cursor-pointer transition-colors ${
                      on
                        ? 'border-[var(--accent)] bg-[var(--bg-active)]'
                        : 'border-[var(--border)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <div className="text-[12px] text-[var(--text-primary)]">{o.label}</div>
                    {o.description && <div className="text-[11px] text-[var(--text-muted)]">{o.description}</div>}
                  </button>
                );
              })}
            </div>
            {q.allow_other && (
              <input
                type={q.secret ? 'password' : 'text'}
                value={other[q.id] ?? ''}
                onChange={(e) => setOther((prev) => ({ ...prev, [q.id]: e.target.value }))}
                placeholder="Or type your own answer…"
                className="w-full h-7 px-2 rounded-md border border-[var(--border)] bg-transparent text-[12px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
              />
            )}
          </div>
        );
      })}
      <div className="flex gap-1.5 pt-0.5">
        <button
          onClick={submit}
          disabled={!complete}
          className="h-6 px-2 rounded-md text-[11px] bg-[var(--accent)] text-white cursor-pointer hover:opacity-95 disabled:opacity-40 disabled:cursor-default"
        >
          Send answer
        </button>
        <button
          onClick={() => onDismiss(request)}
          className="h-6 px-2 rounded-md text-[11px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] cursor-pointer"
        >
          Skip
        </button>
      </div>
    </div>
  );
};
