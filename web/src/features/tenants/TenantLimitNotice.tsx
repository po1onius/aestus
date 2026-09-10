import type { TenantLimits } from "../../types";

export function TenantLimitNotice({ limits, loading, resource }: {
  limits: TenantLimits | null;
  loading: boolean;
  resource: "users" | "keys";
}) {
  return <p className="mb-4 text-sm text-slate-500" role="status">
    {loading ? "正在加载租户限制…" : !limits ? "租户限制加载失败，请刷新页面。" : resource === "users"
      ? `租户用户数上限：${limits.max_users ?? "不限制"}。包含 owner 和停用用户；达到上限后无法新增。`
      : `每用户网关 Key 上限：${limits.max_gateway_keys_per_user ?? "不限制"}。跨 Provider、分组合计；停用和失效 Key 仍计数，删除才释放名额。`}
  </p>;
}
