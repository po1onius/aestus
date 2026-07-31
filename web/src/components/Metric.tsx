import { cx, metricClass } from "../lib/ui";

interface MetricProps {
  label: string;
  value: string;
  tone?: "good" | "warn";
  title?: string;
}

export function Metric({ label, value, tone, title }: MetricProps) {
  return (
    <div
      className={cx(
        metricClass,
        tone === "good"
          ? "border-emerald-200 bg-emerald-50/40 dark:border-emerald-900 dark:bg-emerald-950/30"
          : tone === "warn"
            ? "border-amber-200 bg-amber-50/50 dark:border-amber-900 dark:bg-amber-950/30"
            : "border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900",
      )}
      title={title}
    >
      <span className="block truncate text-xs font-medium text-slate-500 dark:text-slate-400">{label}</span>
      <strong
        className={cx(
          "mt-1.5 block truncate text-xl font-semibold tracking-tight",
          tone === "good"
            ? "text-emerald-800 dark:text-emerald-300"
            : tone === "warn"
              ? "text-amber-800 dark:text-amber-300"
              : "text-slate-950 dark:text-slate-100",
        )}
      >
        {value}
      </strong>
    </div>
  );
}
