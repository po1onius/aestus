import { AnimatePresence } from "motion/react";
import { type FormEvent, useEffect, useState } from "react";
import { Boxes, Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { requestJson } from "../api/client";
import { tenantsPath } from "../config";
import {
  TenantCodeDialog,
  type TenantCodeAction,
} from "../features/tenants/TenantCodeDialog";
import { TenantCreateDialog } from "../features/tenants/TenantCreateDialog";
import { TenantResourcesDialog } from "../features/tenants/TenantResourcesDialog";
import { showErrorToast } from "../lib/errors";
import {
  buttonDangerSolid,
  buttonPrimary,
  buttonSecondary,
  panelHeaderClass,
  panelTitleClass,
  spinnerClass,
} from "../lib/ui";
import type { TenantSummary } from "../types";

interface TenantsPageProps {
  token: string;
  refreshSignal: number;
}

interface TenantCodeDialogState {
  tenant: TenantSummary;
  action: TenantCodeAction;
}

export function TenantsPage({ token, refreshSignal }: TenantsPageProps) {
  const [tenants, setTenants] = useState<TenantSummary[]>([]);
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [updatingId, setUpdatingId] = useState<string | null>(null);
  const [codeDialog, setCodeDialog] = useState<TenantCodeDialogState | null>(null);
  const [resourcesTenant, setResourcesTenant] = useState<TenantSummary | null>(null);

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
        body: JSON.stringify({ id: name.trim(), password: password || null }),
      }, token);
      setName("");
      setPassword("");
      setCreateOpen(false);
      toast.success(password ? "租户和 owner 已创建" : "租户已创建");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户创建失败", error);
    } finally {
      setSaving(false);
    }
  }

  function openCreateDialog() {
    setName("");
    setPassword("");
    setCreateOpen(true);
  }

  function closeCreateDialog() {
    if (saving) return;
    setName("");
    setPassword("");
    setCreateOpen(false);
  }

  async function toggleTenant(tenant: TenantSummary) {
    setUpdatingId(tenant.id);
    try {
      await requestJson(`${tenantsPath}/${encodeURIComponent(tenant.id)}/status`, {
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

  function openCodeDialog(tenant: TenantSummary, action: TenantCodeAction) {
    setCodeDialog({ tenant, action });
  }

  function closeCodeDialog() {
    if (updatingId === codeDialog?.tenant.id) return;
    setCodeDialog(null);
  }

  async function regenerateCode() {
    if (!codeDialog || codeDialog.action !== "regenerate") return;
    const { tenant } = codeDialog;
    setUpdatingId(tenant.id);
    try {
      await requestJson<TenantSummary>(`${tenantsPath}/${encodeURIComponent(tenant.id)}/code`, {
        method: "POST",
      }, token);
      toast.success(tenant.code ? "新租户码已生成" : "租户码已生成");
      await loadTenants();
      setCodeDialog(null);
    } catch (error) {
      showErrorToast("租户码生成失败", error);
    } finally {
      setUpdatingId(null);
    }
  }

  async function revokeCode() {
    if (!codeDialog || codeDialog.action !== "revoke") return;
    const { tenant } = codeDialog;
    setUpdatingId(tenant.id);
    try {
      await requestJson(`${tenantsPath}/${encodeURIComponent(tenant.id)}/code`, { method: "DELETE" }, token);
      toast.success("租户码已撤销");
      await loadTenants();
      setCodeDialog(null);
    } catch (error) {
      showErrorToast("租户码撤销失败", error);
    } finally {
      setUpdatingId(null);
    }
  }

  return (
    <div className="grid gap-5">
      <section className="overflow-hidden rounded-xl border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
        <div className={panelHeaderClass}>
          <div>
            <h2 className={panelTitleClass}>租户列表</h2>
          </div>
          <button className={buttonPrimary} type="button" onClick={openCreateDialog}>
            <Plus size={17} />
            添加租户
          </button>
        </div>
        {loading ? (
          <div className="flex items-center justify-center gap-2 p-10 text-sm text-slate-500"><Loader2 className={spinnerClass} size={18} />正在加载租户</div>
        ) : tenants.length === 0 ? (
          <div className="p-10 text-center text-sm text-slate-500">还没有租户</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[900px] text-left text-sm">
              <thead className="border-b border-slate-200 bg-slate-50 text-slate-500 dark:border-slate-800 dark:bg-slate-950/50">
                <tr><th className="px-4 py-3 font-medium">租户</th><th className="px-4 py-3 font-medium">租户码</th><th className="px-4 py-3 font-medium">状态</th><th className="px-4 py-3 text-right font-medium">操作</th></tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {tenants.map((tenant) => {
                  const updating = updatingId === tenant.id;
                  return (
                    <tr key={tenant.id}>
                      <td className="px-4 py-3 font-medium">{tenant.id}</td>
                      <td className="px-4 py-3 font-mono">{tenant.code ?? <span className="font-sans text-slate-400">已撤销</span>}</td>
                      <td className="px-4 py-3"><span className={tenant.enabled ? "text-emerald-600" : "text-slate-400"}>{tenant.enabled ? "启用" : "停用"}</span></td>
                      <td className="px-4 py-3"><div className="flex justify-end gap-2">
                        <button className={buttonSecondary} type="button" disabled={updating} onClick={() => setResourcesTenant(tenant)}><Boxes size={15} />查看资源</button>
                        <button className={buttonSecondary} type="button" disabled={updating} onClick={() => openCodeDialog(tenant, "regenerate")}><RefreshCw size={15} />{tenant.code ? "修改租户码" : "设置租户码"}</button>
                        {tenant.code && <button className={buttonDangerSolid} type="button" disabled={updating} onClick={() => openCodeDialog(tenant, "revoke")}><Trash2 size={15} />撤销租户码</button>}
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
      <AnimatePresence>
        {createOpen && (
          <TenantCreateDialog
            name={name}
            password={password}
            saving={saving}
            onNameChange={setName}
            onPasswordChange={setPassword}
            onSubmit={createTenant}
            onClose={closeCreateDialog}
          />
        )}
        {codeDialog && (
          <TenantCodeDialog
            key={`${codeDialog.tenant.id}-${codeDialog.action}`}
            tenant={codeDialog.tenant}
            action={codeDialog.action}
            pending={updatingId === codeDialog.tenant.id}
            onConfirm={codeDialog.action === "revoke" ? revokeCode : regenerateCode}
            onClose={closeCodeDialog}
          />
        )}
        {resourcesTenant && (
          <TenantResourcesDialog
            key={resourcesTenant.id}
            tenant={resourcesTenant}
            token={token}
            onClose={() => setResourcesTenant(null)}
          />
        )}
      </AnimatePresence>
    </div>
  );
}
