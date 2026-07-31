import { statusOptions } from "../config";
import { cx, statusBadgeBase } from "../lib/ui";
import type { AccountStatus } from "../types";

const statusToneClasses: Record<string, string> = {
  active: "bg-emerald-50 text-emerald-700 ring-emerald-600/20 dark:bg-emerald-950/60 dark:text-emerald-300 dark:ring-emerald-700/50",
  valid: "bg-emerald-50 text-emerald-700 ring-emerald-600/20 dark:bg-emerald-950/60 dark:text-emerald-300 dark:ring-emerald-700/50",
  success: "bg-emerald-50 text-emerald-700 ring-emerald-600/20 dark:bg-emerald-950/60 dark:text-emerald-300 dark:ring-emerald-700/50",
  abnormal: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  failed: "bg-red-50 text-red-700 ring-red-600/20 dark:bg-red-950/60 dark:text-red-300 dark:ring-red-700/50",
  disabled: "bg-slate-100 text-slate-600 ring-slate-500/20 dark:bg-slate-800 dark:text-slate-300 dark:ring-slate-600/50",
  pending: "bg-slate-100 text-slate-600 ring-slate-500/20 dark:bg-slate-800 dark:text-slate-300 dark:ring-slate-600/50",
  unauthorized: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  unavailable: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  invalid: "bg-red-50 text-red-700 ring-red-600/20 dark:bg-red-950/60 dark:text-red-300 dark:ring-red-700/50",
  error: "bg-red-50 text-red-700 ring-red-600/20 dark:bg-red-950/60 dark:text-red-300 dark:ring-red-700/50",
};

export function StatusBadge({ status }: { status: AccountStatus }) {
  const label =
    status === "success"
      ? "成功"
      : status === "abnormal"
        ? "异常"
        : status === "failed"
          ? "失败"
          : status === "error"
            ? "错误"
            : status === "pending"
              ? "未完成"
              : (statusOptions.find((item) => item.value === status)?.label ?? status);
  return (
    <span className={cx(statusBadgeBase, statusToneClasses[status] ?? statusToneClasses.pending)}>
      {label}
    </span>
  );
}
