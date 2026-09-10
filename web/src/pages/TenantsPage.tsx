import { RowActions } from "../components/RowActions";
import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";
import { Boxes, Loader2, Plus, Power, RefreshCw, Settings2, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useRequestScope } from "../api/useRequestScope";
import { tenantsPath } from "../config";
import {
  TenantCodeDialog,
  type TenantCodeAction,
} from "../features/tenants/TenantCodeDialog";
import { TenantCreateDialog, type CreateTenantInput } from "../features/tenants/TenantCreateDialog";
import { TenantLimitsDialog } from "../features/tenants/TenantLimitsDialog";
import { TenantResourcesDialog } from "../features/tenants/TenantResourcesDialog";
import { showErrorToast } from "../lib/errors";
import {
  buttonPrimary,
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
  const { requestJson, beginRequest } = useRequestScope(token);
  const [tenants, setTenants] = useState<TenantSummary[]>([]);
  const [createOpen, setCreateOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [updatingId, setUpdatingId] = useState<string | null>(null);
  const [codeDialog, setCodeDialog] = useState<TenantCodeDialogState | null>(null);
  const [limitsTenant, setLimitsTenant] = useState<TenantSummary | null>(null);
  const [resourcesTenant, setResourcesTenant] = useState<TenantSummary | null>(null);

  useEffect(() => {
    void loadTenants();
  }, [token, refreshSignal]);

  async function loadTenants() {
    const request = beginRequest("tenants");
    setLoading(true);
    try {
      const result = await requestJson<TenantSummary[]>(tenantsPath, { signal: request.signal }, token);
      if (request.isCurrent()) setTenants(result);
    } catch (error) {
      if (request.isCurrent()) showErrorToast("租户加载失败", error);
    } finally {
      if (request.isCurrent()) setLoading(false);
    }
  }

  async function createTenant(input: CreateTenantInput) {
    setSaving(true);
    try {
      await requestJson<TenantSummary>(tenantsPath, {
        method: "POST",
        body: JSON.stringify(input),
      }, token);
      setCreateOpen(false);
      toast.success(input.password ? "租户和 owner 已创建" : "租户已创建");
      await loadTenants();
    } catch (error) {
      showErrorToast("租户创建失败", error);
    } finally {
      setSaving(false);
    }
  }

  function openCreateDialog() {
    setCreateOpen(true);
  }

  function closeCreateDialog() {
    if (saving) return;
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
            <table className="w-full min-w-[1350px] text-left text-sm">
              <thead className="border-b border-slate-200 bg-slate-50 text-slate-500 dark:border-slate-800 dark:bg-slate-950/50">
                <tr><th className="px-4 py-3 font-medium">租户</th><th className="px-4 py-3 font-medium">租户码</th><th className="px-4 py-3 font-medium">状态</th><th className="px-4 py-3 font-medium">用户数 / 上限</th><th className="px-4 py-3 font-medium">上游资源数 / 上限</th><th className="px-4 py-3 font-medium">分组数 / 上限</th><th className="px-4 py-3 font-medium">每用户 Key 上限</th><th className="px-4 py-3 font-medium">Owner 上传 WASM</th><th className="w-20 px-4 py-3 text-right font-medium">操作</th></tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {tenants.map((tenant) => {
                  const updating = updatingId === tenant.id;
                  return (
                    <tr key={tenant.id}>
                      <td className="px-4 py-3 font-medium">{tenant.id}</td>
                      <td className="px-4 py-3 font-mono">{tenant.code ?? <span className="font-sans text-slate-400">已撤销</span>}</td>
                      <td className="px-4 py-3"><span className={tenant.enabled ? "text-emerald-600" : "text-slate-400"}>{tenant.enabled ? "启用" : "停用"}</span></td>
                      <td className="px-4 py-3">{tenant.user_count} / {tenant.max_users ?? "不限制"}</td>
                      <td className="px-4 py-3">{tenant.resource_count} / {tenant.max_resources ?? "不限制"}</td>
                      <td className="px-4 py-3">{tenant.provider_group_count} / {tenant.max_provider_groups ?? "不限制"}</td>
                      <td className="px-4 py-3">{tenant.max_gateway_keys_per_user ?? "不限制"}</td>
                      <td className="px-4 py-3">{tenant.owner_can_upload_wasm ? "允许" : "禁止"}</td>
                      <td className="px-4 py-3">
                        <RowActions
                          resourceLabel={tenant.id}
                          busy={updating}
                          actions={[
                            {
                              id: "limits", label: "设置限制", icon: Settings2,
                              opensDialog: true, onSelect: () => setLimitsTenant(tenant),
                            },
                            {
                              id: "view-resources", label: "查看资源", icon: Boxes,
                              onSelect: () => setResourcesTenant(tenant),
                              opensDialog: true,
                            },
                            {
                              id: "regenerate-code", label: tenant.code ? "修改租户码" : "设置租户码", icon: RefreshCw,
                              opensDialog: true, onSelect: () => openCodeDialog(tenant, "regenerate"),
                            },
                            {
                              id: "toggle-enabled", label: tenant.enabled ? "停用" : "启用", icon: Power,
                              onSelect: () => void toggleTenant(tenant),
                            },
                            {
                              id: "revoke-code", label: "撤销租户码", icon: Trash2,
                              hidden: !tenant.code, danger: true, opensDialog: true,
                              onSelect: () => openCodeDialog(tenant, "revoke"),
                            },
                          ]}
                        />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <AnimatePresence>
        {limitsTenant && <TenantLimitsDialog key={limitsTenant.id} tenant={limitsTenant} token={token}
          onClose={() => setLimitsTenant(null)}
          onSaved={() => { setLimitsTenant(null); void loadTenants(); }} />}
        {createOpen && (
          <TenantCreateDialog
            saving={saving}
            onCreate={createTenant}
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
