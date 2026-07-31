import { Plus, Trash2 } from "lucide-react";
import { buttonSecondary, iconButton, inputClass, textareaClass } from "../lib/ui";
import type { OverrideEntry } from "../types";

interface OverrideKvEditorProps {
  title: string;
  rows: OverrideEntry[];
  disabled: boolean;
  onAdd: () => void;
  onChange: (id: string, field: "key" | "value", value: string) => void;
  onRemove: (id: string) => void;
}

export function OverrideKvEditor({
  title,
  rows,
  disabled,
  onAdd,
  onChange,
  onRemove,
}: OverrideKvEditorProps) {
  return (
    <section className="overflow-hidden rounded-xl border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-slate-200 bg-slate-50 px-4 py-3 dark:border-slate-800 dark:bg-slate-950/70">
        <h2 className="text-sm font-semibold text-slate-900 dark:text-slate-100">{title}</h2>
        <button type="button" className={buttonSecondary} onClick={onAdd} disabled={disabled}>
          <Plus size={18} />
          添加
        </button>
      </div>
      {rows.length === 0 ? (
        <div className="flex min-h-28 items-center justify-center px-5 text-sm text-slate-500 dark:text-slate-400">
          <span>暂无覆盖项</span>
        </div>
      ) : (
        <div>
          <div className="hidden grid-cols-[minmax(8rem,0.45fr)_minmax(12rem,1fr)_2.25rem] gap-3 border-b border-slate-200 bg-slate-50 px-4 py-2 text-xs font-semibold uppercase tracking-wide text-slate-500 dark:border-slate-800 dark:bg-slate-950/70 dark:text-slate-400 sm:grid">
            <span>Key</span>
            <span>Value</span>
            <span>操作</span>
          </div>
          {rows.map((row) => (
            <div className="grid gap-3 border-b border-slate-100 px-4 py-3 last:border-b-0 dark:border-slate-800 sm:grid-cols-[minmax(8rem,0.45fr)_minmax(12rem,1fr)_2.25rem]" key={row.id}>
              <input
                className={inputClass}
                value={row.key}
                onChange={(event) => onChange(row.id, "key", event.target.value)}
                placeholder="名称"
                autoComplete="off"
                disabled={disabled}
              />
              <textarea
                className={`${textareaClass} min-h-16`}
                value={row.value}
                onChange={(event) => onChange(row.id, "value", event.target.value)}
                placeholder="JSON 值"
                rows={2}
                disabled={disabled}
              />
              <button
                type="button"
                className={iconButton}
                onClick={() => onRemove(row.id)}
                disabled={disabled}
                title="删除"
                aria-label={`删除 ${title} 覆盖项`}
              >
                <Trash2 size={18} />
              </button>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
