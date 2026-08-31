import { FormEvent, useEffect, useState } from "react";
import { Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { requestJson } from "../api/client";
import { tenantsPath } from "../config";
import { showErrorToast } from "../lib/errors";
import { buttonDangerSolid, buttonPrimary, buttonSecondary, inputClass, spinnerClass } from "../lib/ui";
import type { TenantSummary } from "../types";

interface TenantsPageProps {
  token: string;
  refreshSignal: number;
}

export function TenantsPage({ token, refreshSignal }: TenantsPageProps) {
  const [tenants, setTenants] = useState<TenantSummary[]>([]);
  const [name, setName] = useState("");
  const [code, setCode] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [updatingId, setUpdatingId] = useState<string | null>(null);

  useEffect(() => {
    void loadTenants();
  }, [token, refreshSignal]);

  async function loadTenants() {
    setLoading(true);
    try {
      setTenants(await requestJson<TenantSummary[]>(tenantsPath, undefined, token));
    } catch (error) {
      showErrorToast("租户加载失败", error);
    } finally {
      setLoading(false);
    }
  }

  async function createTenant(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    try {
      await requestJson<TenantSummary>(tenantsPath, {
        method: "POST",
        body: JSON.stringify({ name: name.trim(), code: code.trim() }),
      }, token);
      setName("");
      setCode("");
      toast.success("租户已创建");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户创建失败", error);
    } finally {
      setSaving(false);
    }
  }

  async function toggleTenant(tenant: TenantSummary) {
    setUpdatingId(tenant.id);
    try {
      await requestJson(`${tenantsPath}/${tenant.id}/status`, {
        method: "PUT",
        body: JSON.stringify({ enabled: !tenant.enabled }),
      }, token);
      toast.success(tenant.enabled ? "租户已停用" : "租户已启用");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户状态更新失败", error);
    } finally {
      setUpdatingId(null);
    }
  }

  async function replaceCode(tenant: TenantSummary) {
    const nextCode = window.prompt("输入新的租户码", tenant.code ?? "")?.trim();
    if (!nextCode || nextCode === tenant.code) return;
    setUpdatingId(tenant.id);
    try {
      await requestJson<TenantSummary>(`${tenantsPath}/${tenant.id}/code`, {
        method: "POST",
        body: JSON.stringify({ code: nextCode }),
      }, token);
      toast.success("租户码已更新");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户码更新失败", error);
    } finally {
      setUpdatingId(null);
    }
  }

  async function revokeCode(tenant: TenantSummary) {
    if (!window.confirm(`确认撤销租户“${tenant.name}”的租户码？撤销后将不能继续公开注册。`)) return;
    setUpdatingId(tenant.id);
    try {
      await requestJson(`${tenantsPath}/${tenant.id}/code`, { method: "DELETE" }, token);
      toast.success("租户码已撤销");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户码撤销失败", error);
    } finally {
      setUpdatingId(null);
    }
  }

  return (
    <div className="grid gap-5">
      <form className="grid gap-3 rounded-xl border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]" onSubmit={createTenant}>
        <input className={inputClass} value={name} onChange={(event) => setName(event.target.value)} placeholder="租户名称" maxLength={128} required />
        <input className={inputClass} value={code} onChange={(event) => setCode(event.target.value)} placeholder="租户码" maxLength={128} required />
        <button className={buttonPrimary} disabled={saving}>
          {saving ? <Loader2 className={spinnerClass} size={17} /> : <Plus size={17} />}
          创建租户
        </button>
      </form>

      <section className="overflow-hidden rounded-xl border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
        {loading ? (
          <div className="flex items-center justify-center gap-2 p-10 text-sm text-slate-500"><Loader2 className={spinnerClass} size={18} />正在加载租户</div>
        ) : tenants.length === 0 ? (
          <div className="p-10 text-center text-sm text-slate-500">还没有租户</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[780px] text-left text-sm">
              <thead className="border-b border-slate-200 bg-slate-50 text-slate-500 dark:border-slate-800 dark:bg-slate-950/50">
                <tr><th className="px-4 py-3 font-medium">租户</th><th className="px-4 py-3 font-medium">租户码</th><th className="px-4 py-3 font-medium">状态</th><th className="px-4 py-3 text-right font-medium">操作</th></tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {tenants.map((tenant) => {
                  const updating = updatingId === tenant.id;
                  return (
                    <tr key={tenant.id}>
                      <td className="px-4 py-3"><div className="font-medium">{tenant.name}</div><div className="text-xs text-slate-400">{tenant.id}</div></td>
                      <td className="px-4 py-3 font-mono">{tenant.code ?? <span className="font-sans text-slate-400">已撤销</span>}</td>
                      <td className="px-4 py-3"><span className={tenant.enabled ? "text-emerald-600" : "text-slate-400"}>{tenant.enabled ? "启用" : "停用"}</span></td>
                      <td className="px-4 py-3"><div className="flex justify-end gap-2">
                        <button className={buttonSecondary} type="button" disabled={updating} onClick={() => void replaceCode(tenant)}><RefreshCw size={15} />{tenant.code ? "修改租户码" : "设置租户码"}</button>
                        {tenant.code && <button className={buttonDangerSolid} type="button" disabled={updating} onClick={() => void revokeCode(tenant)}><Trash2 size={15} />撤销租户码</button>}
                        <button className={buttonSecondary} type="button" disabled={updating} onClick={() => void toggleTenant(tenant)}>{updating ? <Loader2 className={spinnerClass} size={15} /> : null}{tenant.enabled ? "停用" : "启用"}</button>
                      </div></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
