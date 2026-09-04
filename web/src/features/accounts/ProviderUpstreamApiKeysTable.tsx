import { Loader2, Settings, Trash2 } from "lucide-react";
import { RuntimeBadge } from "../../components/RuntimeBadge";
import { StatusBadge } from "../../components/StatusBadge";
import { formatOptionalDateTime } from "../../lib/format";
import type { ProviderAccess } from "../group-access/access";
import {
  actionStackClass,
  buttonSmall,
  buttonSmallDanger,
  cellMainClass,
  cellNoteClass,
  disabledRowClass,
  entryStackClass,
  entryTitleClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../../lib/ui";
import type {
  ProviderGroup,
  ProviderUpstreamApiKey,
  UpstreamApiKeyProvider,
} from "../../types";
import { enabledToggleLabel } from "./utils";
import { ProviderGroupCell } from "./ProviderGroupCell";

interface ProviderUpstreamApiKeysTableProps {
  access: ProviderAccess;
  provider: UpstreamApiKeyProvider;
  providerLabel: string;
  apiKeys: ProviderUpstreamApiKey[];
  groups: ProviderGroup[];
  groupUpdatingId: string | null;
  enabledUpdatingId: string | null;
  deletingId: string | null;
  onUpdateEnabled: (apiKey: ProviderUpstreamApiKey, enabled: boolean) => void;
  onUpdateGroup: (apiKey: ProviderUpstreamApiKey, groupId: string) => void;
  onOpenOverride: (apiKey: ProviderUpstreamApiKey) => void;
  onDelete: (apiKey: ProviderUpstreamApiKey) => void;
}

/** Provider 官方 API Key 共用表格，统一展示探活、调度和请求覆盖状态。 */
export function ProviderUpstreamApiKeysTable({
  access,
  provider,
  providerLabel,
  apiKeys,
  groups,
  groupUpdatingId,
  enabledUpdatingId,
  deletingId,
  onUpdateEnabled,
  onUpdateGroup,
  onOpenOverride,
  onDelete,
}: ProviderUpstreamApiKeysTableProps) {
  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[92rem]`}>
        <thead>
          <tr>
            <th>{providerLabel} 官方 Key</th>
            <th>所在组</th>
            <th>Base URL</th>
            <th>状态</th>
            <th>调度</th>
            <th>检测</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {apiKeys.map((apiKey) => {
            const canViewOverride =
              access.has(apiKey.group?.id, "official_api_key.override.view") &&
              apiKey.override !== null;
            const busy =
              deletingId === apiKey.id ||
              enabledUpdatingId === apiKey.id ||
              groupUpdatingId === apiKey.id;
            const probePending = apiKey.runtime.next_probe_at !== null;

            return (
              <tr key={apiKey.id} className={apiKey.enabled ? undefined : disabledRowClass}>
                <td>
                  <div className={entryStackClass}>
                    <strong className={entryTitleClass} title={apiKey.masked_api_key}>{apiKey.masked_api_key}</strong>
                    <span className={cellNoteClass} title={apiKey.id}>{apiKey.id}</span>
                  </div>
                </td>
                <td>
                  {access.isOwner ? (
                    <ProviderGroupCell
                      resourceLabel={apiKey.masked_api_key}
                      provider={provider}
                      group={apiKey.group}
                      groups={groups}
                      disabled={busy}
                      onChange={(groupId) => onUpdateGroup(apiKey, groupId)}
                    />
                  ) : (
                    <span className={cellMainClass}>{apiKey.group?.name ?? "未分组"}</span>
                  )}
                </td>
                <td>
                  <div className={cellMainClass} title={apiKey.base_url}>
                    {apiKey.base_url}
                  </div>
                </td>
                <td>
                  <div className="grid gap-1.5">
                    <StatusBadge status={probePending ? "unavailable" : "valid"} />
                    <p className={cellNoteClass}>管理员：{apiKey.enabled ? "已启用" : "已禁用"}</p>
                    {apiKey.error && (
                      <p className={`${cellNoteClass} max-w-40 text-red-700 dark:text-red-400`} title={apiKey.error}>
                        {apiKey.error}
                      </p>
                    )}
                  </div>
                </td>
                <td>
                  <RuntimeBadge runtime={apiKey.runtime} />
                  <p className={cellNoteClass}>
                    并发：{apiKey.runtime.inflight_count} ·{" "}
                    {apiKey.runtime.runtime_ready ? "runtime 就绪" : "runtime 未就绪"}
                  </p>
                  <p className={cellNoteClass}>
                    运行态：{apiKey.runtime.runtime_exists ? "已发布" : "未发布"}
                  </p>
                </td>
                <td>
                  <div className={cellMainClass}>
                    {probePending
                      ? "等待探活"
                      : apiKey.runtime.runtime_ready
                        ? "可调度"
                        : "未发布"}
                  </div>
                  <p className={cellNoteClass}>
                    下次探活：{formatOptionalDateTime(apiKey.runtime.next_probe_at)}
                  </p>
                </td>
                <td>
                  <div className={actionStackClass}>
                    {access.isOwner && <button
                      className={buttonSmall}
                      disabled={busy}
                      onClick={() => onUpdateEnabled(apiKey, !apiKey.enabled)}
                      title={apiKey.enabled ? "禁用官方 Key 调度" : "启用官方 Key 调度"}
                    >
                      {enabledUpdatingId === apiKey.id ? (
                        <Loader2 className={spinnerClass} size={14} />
                      ) : (
                        enabledToggleLabel(apiKey.enabled)
                      )}
                    </button>}
                    <button
                      className={buttonSmall}
                      disabled={busy || !canViewOverride}
                      onClick={() => onOpenOverride(apiKey)}
                      title={canViewOverride ? "查看请求覆盖" : "未获得查看官方 Key 覆盖权限"}
                    >
                      <Settings size={14} />
                      覆盖
                    </button>
                    {access.isOwner && <button
                      className={buttonSmallDanger}
                      disabled={busy}
                      onClick={() => onDelete(apiKey)}
                      title="删除官方 Key"
                    >
                      {deletingId === apiKey.id ? (
                        <Loader2 className={spinnerClass} size={14} />
                      ) : (
                        <Trash2 size={14} />
                      )}
                      删除
                    </button>}
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
