import { cx, statusBadgeBase } from "../lib/ui";
import type { GptAccountRuntime, RuntimeViewState } from "../types";

const runtimeToneClasses: Record<RuntimeViewState, string> = {
  ready: "bg-emerald-50 text-emerald-700 ring-emerald-600/20 dark:bg-emerald-950/60 dark:text-emerald-300 dark:ring-emerald-700/50",
  token_refresh_pending: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  quota_limited: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  pending_probe: "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-950/60 dark:text-amber-300 dark:ring-amber-700/50",
  missing: "bg-red-50 text-red-700 ring-red-600/20 dark:bg-red-950/60 dark:text-red-300 dark:ring-red-700/50",
  not_runtime: "bg-slate-100 text-slate-600 ring-slate-500/20 dark:bg-slate-800 dark:text-slate-300 dark:ring-slate-600/50",
};

export function RuntimeBadge({
  runtime,
}: {
  runtime: Pick<GptAccountRuntime, "runtime_state">;
}) {
  return (
    <span className={cx(statusBadgeBase, runtimeToneClasses[runtime.runtime_state])}>
      {runtimeStateLabel(runtime.runtime_state)}
    </span>
  );
}

function runtimeStateLabel(state: RuntimeViewState) {
  switch (state) {
    case "ready":
      return "就绪";
    case "token_refresh_pending":
      return "待刷新";
    case "quota_limited":
      return "额度限制";
    case "pending_probe":
      return "待探活";
    case "not_runtime":
      return "未发布";
    case "missing":
    default:
      return "未加载";
  }
}
