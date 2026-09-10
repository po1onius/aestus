import { useState, type FormEvent } from "react";
import { Loader2, Save } from "lucide-react";
import { toast } from "sonner";
import { useRequestScope } from "../../api/useRequestScope";
import { Modal } from "../../components/Modal";
import { tenantsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import { buttonPrimary, spinnerClass } from "../../lib/ui";
import type { TenantSummary } from "../../types";
import { TenantLimitsFields, parseTenantLimits, tenantLimitsDraft } from "./TenantLimitsFields";

export function TenantLimitsDialog({ tenant, token, onSaved, onClose }: {
  tenant: TenantSummary;
  token: string;
  onSaved: () => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(() => tenantLimitsDraft(tenant));
  const [saving, setSaving] = useState(false);
  const { requestJson, isActiveAuthToken } = useRequestScope(token);
  const limits = parseTenantLimits(draft);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!limits || saving) return;
    setSaving(true);
    try {
      await requestJson(`${tenantsPath}/${encodeURIComponent(tenant.id)}/limits`, {
        method: "PUT", body: JSON.stringify(limits),
      });
      toast.success("租户限制已更新");
      onSaved();
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("租户限制更新失败", error);
    } finally {
      if (isActiveAuthToken(token)) setSaving(false);
    }
  }
  return <Modal titleId="tenantLimitsTitle" title={`设置限制 · ${tenant.id}`}
    description={`当前用户数：${tenant.user_count}。调低上限后保留现有资源，新增时执行限制。`}
    className="max-w-lg" closeDisabled={saving} onClose={onClose}>
    <form className="grid gap-4" onSubmit={submit}>
      <TenantLimitsFields value={draft} onChange={setDraft} disabled={saving} />
      <button className={buttonPrimary} disabled={saving || !limits}>
        {saving ? <Loader2 className={spinnerClass} size={17} /> : <Save size={17} />}
        {saving ? "正在保存" : "保存限制"}
      </button>
    </form>
  </Modal>;
}
