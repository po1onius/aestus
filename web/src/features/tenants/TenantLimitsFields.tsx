import { fieldLabel, fieldStack, inputClass } from "../../lib/ui";
import type { TenantLimits } from "../../types";

export interface TenantLimitsDraft {
  maxUsers: string;
  maxResources: string;
  maxProviderGroups: string;
  maxGatewayKeys: string;
  ownerCanUploadWasm: boolean;
}

export function tenantLimitsDraft(limits?: TenantLimits): TenantLimitsDraft {
  return {
    maxUsers: limits?.max_users?.toString() ?? "",
    maxResources: limits?.max_resources?.toString() ?? "",
    maxProviderGroups: limits?.max_provider_groups?.toString() ?? "",
    maxGatewayKeys: limits?.max_gateway_keys_per_user?.toString() ?? "",
    ownerCanUploadWasm: limits?.owner_can_upload_wasm ?? false,
  };
}

export function parseTenantLimits(draft: TenantLimitsDraft): TenantLimits | null {
  const valid = (value: string) => value === "" || (/^\d+$/.test(value) && Number(value) <= 2147483647);
  if (!valid(draft.maxUsers) || !valid(draft.maxResources) || !valid(draft.maxProviderGroups) || !valid(draft.maxGatewayKeys)) return null;
  return {
    max_users: draft.maxUsers === "" ? null : Number(draft.maxUsers),
    max_resources: draft.maxResources === "" ? null : Number(draft.maxResources),
    max_provider_groups: draft.maxProviderGroups === "" ? null : Number(draft.maxProviderGroups),
    max_gateway_keys_per_user: draft.maxGatewayKeys === "" ? null : Number(draft.maxGatewayKeys),
    owner_can_upload_wasm: draft.ownerCanUploadWasm,
  };
}

export function TenantLimitsFields({ value, onChange, disabled }: {
  value: TenantLimitsDraft;
  onChange: (value: TenantLimitsDraft) => void;
  disabled: boolean;
}) {
  return <fieldset disabled={disabled} className="grid gap-4">
    <label className={fieldStack}>
      <span className={fieldLabel}>租户用户数上限</span>
      <input className={inputClass} type="number" min={0} max={2147483647} step={1}
        placeholder="不限制" value={value.maxUsers}
        onChange={event => onChange({ ...value, maxUsers: event.target.value })} />
      <span className="text-xs text-slate-500">包含 owner 和停用用户。留空不限制，0 禁止新增。</span>
    </label>
    <label className={fieldStack}>
      <span className={fieldLabel}>每用户网关 Key 上限</span>
      <input className={inputClass} type="number" min={0} max={2147483647} step={1}
        placeholder="不限制" value={value.maxGatewayKeys}
        onChange={event => onChange({ ...value, maxGatewayKeys: event.target.value })} />
      <span className="text-xs text-slate-500">owner 和每位用户分别计数，跨 Provider、分组合计；停用和失效 Key 也计数，删除才释放名额。留空不限制，0 禁止新增。</span>
    </label>
    <label className={fieldStack}>
      <span className={fieldLabel}>上游资源总数上限</span>
      <input className={inputClass} type="number" min={0} max={2147483647} step={1}
        placeholder="不限制" value={value.maxResources}
        onChange={event => onChange({ ...value, maxResources: event.target.value })} />
      <span className="text-xs text-slate-500">跨所有 Provider 合计 OAuth 账号和官方 API Key，不包含网关 Key；停用、失效和未分组资源也计数，删除才释放名额。留空不限制，0 禁止新增。</span>
    </label>
    <label className={fieldStack}>
      <span className={fieldLabel}>分组总数上限</span>
      <input className={inputClass} type="number" min={0} max={2147483647} step={1}
        placeholder="不限制" value={value.maxProviderGroups}
        onChange={event => onChange({ ...value, maxProviderGroups: event.target.value })} />
      <span className="text-xs text-slate-500">跨所有 Provider 合计；停用和空分组也计数，删除才释放名额。留空不限制，0 禁止新增。</span>
    </label>
    <label className="flex items-center gap-2 text-sm">
      <input type="checkbox" checked={value.ownerCanUploadWasm}
        onChange={event => onChange({ ...value, ownerCanUploadWasm: event.target.checked })} />
      允许 owner 上传 WASM 插件
    </label>
    <p className="text-xs text-slate-500">关闭上传后，已有插件和套件仍可使用、管理。</p>
  </fieldset>;
}
