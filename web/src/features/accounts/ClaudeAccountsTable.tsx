import { Loader2, Settings, Trash2 } from "lucide-react";
import { RuntimeBadge } from "../../components/RuntimeBadge";
import { StatusBadge } from "../../components/StatusBadge";
import { formatDateTime, formatOptionalDateTime } from "../../lib/format";
import type { ProviderAccess } from "../group-access/access";
import {
  actionStackClass,
  buttonSmall,
  buttonSmallDanger,
  cellMainClass,
  cellNoteClass,
  cellWrapClass,
  disabledRowClass,
  entryStackClass,
  entryTitleClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../../lib/ui";
import type { ClaudeAccount, ProviderGroup } from "../../types";
import { enabledToggleLabel } from "./utils";
import { ProviderGroupCell } from "./ProviderGroupCell";

interface ClaudeAccountsTableProps {
  access: ProviderAccess;
  accounts: ClaudeAccount[];
  groups: ProviderGroup[];
  groupUpdatingId: string | null;
  enabledUpdatingId: string | null;
  deletingId: string | null;
  onUpdateEnabled: (account: ClaudeAccount, enabled: boolean) => void;
  onUpdateGroup: (account: ClaudeAccount, groupId: string) => void;
  onOpenOverride: (account: ClaudeAccount) => void;
  onDelete: (account: ClaudeAccount) => void;
}

const subscriptionLabels: Record<NonNullable<ClaudeAccount["subscription_type"]>, string> = {
  max: "Max",
  pro: "Pro",
  team: "Team",
  enterprise: "Enterprise",
};

function formatExtraUsage(enabled: boolean | null): string {
  if (enabled === null) {
    return "未返回";
  }
  return enabled ? "已开启" : "未开启";
}

/**
 * Claude OAuth 账号表格只负责账号运行态的展示与操作派发。
 * 数据请求和弹窗状态留在上层，避免表格组件持有第二份服务端状态。
 */
export function ClaudeAccountsTable({
  access,
  accounts,
  groups,
  groupUpdatingId,
  enabledUpdatingId,
  deletingId,
  onUpdateEnabled,
  onUpdateGroup,
  onOpenOverride,
  onDelete,
}: ClaudeAccountsTableProps) {
  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[112rem]`}>
        <thead>
          <tr>
            <th>账号</th>
            <th>所在组</th>
            <th>Anthropic 身份</th>
            <th>状态</th>
            <th>并发</th>
            <th>调度</th>
            <th>凭证</th>
            <th>更新</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {accounts.map((account) => {
            const canViewOverride =
              access.has(account.group?.id, "account.override.view") && account.override !== null;
            return (
            <tr key={account.id} className={account.enabled ? undefined : disabledRowClass}>
              <td>
                <div className={entryStackClass}>
                  <strong className={entryTitleClass} title={account.display_name || account.email || "未返回账号名称"}>
                    {account.display_name || account.email || "未返回账号名称"}
                  </strong>
                  {account.display_name && <span className={cellNoteClass} title={account.email || "未返回邮箱"}>{account.email || "未返回邮箱"}</span>}
                  <span className={cellNoteClass} title={account.id}>{account.id}</span>
                  <span className={cellNoteClass} title={`client_id: ${account.client_id}`}>client_id: {account.client_id}</span>
                </div>
              </td>
              <td>
                {access.isOwner ? (
                  <ProviderGroupCell
                    resourceLabel={account.display_name || account.email || account.id}
                    provider="claude"
                    group={account.group}
                    groups={groups}
                    disabled={
                      deletingId === account.id ||
                      enabledUpdatingId === account.id ||
                      groupUpdatingId === account.id
                    }
                    onChange={(groupId) => onUpdateGroup(account, groupId)}
                  />
                ) : (
                  <span className={cellMainClass}>{account.group?.name ?? "未分组"}</span>
                )}
              </td>
              <td>
                <div className={cellMainClass} title={account.account_uuid || "未返回 account UUID"}>
                  Account：{account.account_uuid || "未返回"}
                </div>
                <p className={cellWrapClass} title={account.organization_uuid || "未返回 organization UUID"}>
                  Organization：{account.organization_uuid || "未返回"}
                </p>
                <p className={cellNoteClass}>
                  订阅：{account.subscription_type ? subscriptionLabels[account.subscription_type] : "未返回"}
                </p>
                <p className={cellWrapClass} title={account.rate_limit_tier || "未返回 rate limit tier"}>
                  Rate limit tier：{account.rate_limit_tier || "未返回"}
                </p>
                <p className={cellNoteClass}>
                  额外用量：{formatExtraUsage(account.has_extra_usage_enabled)} · 计费：{account.billing_type || "未返回"}
                </p>
              </td>
              <td>
                <div className="grid gap-1.5">
                  <StatusBadge status={account.status} />
                  <p className={cellNoteClass}>管理状态：{account.enabled ? "已启用" : "已禁用"}</p>
                  {account.status_reason && (
                    <p className={`${cellNoteClass} max-w-40 text-red-700 dark:text-red-400`} title={account.status_reason}>
                      {account.status_reason}
                    </p>
                  )}
                </div>
              </td>
              <td>
                <div className={cellMainClass}>{account.runtime.inflight_count}</div>
              </td>
              <td>
                <RuntimeBadge runtime={account.runtime} />
                <p className={cellNoteClass}>
                  可调度：{account.runtime.runtime_ready ? "是" : "否"}
                </p>
                <p className={cellNoteClass}>
                  运行态：{account.runtime.runtime_exists ? "已发布" : "未发布"}
                </p>
                {account.runtime.runtime_state === "quota_limited" &&
                  (account.runtime.quota_resets_at || account.quota_resets_at) && (
                    <p className={cellNoteClass}>
                      预计恢复：
                      {formatOptionalDateTime(
                        account.runtime.quota_resets_at || account.quota_resets_at,
                      )}
                    </p>
                  )}
              </td>
              <td>
                <div className={cellMainClass}>
                  Token：{account.runtime.token_usable ? "可用" : "不可用"}
                </div>
                <p className={cellNoteClass}>
                  下次刷新：{formatOptionalDateTime(account.runtime.next_token_refresh_at)}
                </p>
                <p className={cellNoteClass}>
                  Refresh Token 到期：{formatOptionalDateTime(account.refresh_token_expires_at)}
                </p>
              </td>
              <td>
                <div className={cellMainClass}>更新：{formatDateTime(account.updated_at)}</div>
                <p className={cellNoteClass}>创建：{formatDateTime(account.created_at)}</p>
                <p className={cellNoteClass}>
                  订阅创建：{formatOptionalDateTime(account.subscription_created_at)}
                </p>
                <p className={cellNoteClass}>
                  账号创建：{formatOptionalDateTime(account.account_created_at)}
                </p>
              </td>
              <td>
                <div className={actionStackClass}>
                  {access.isOwner && <button
                    className={buttonSmall}
                    disabled={
                      deletingId === account.id ||
                      enabledUpdatingId === account.id ||
                      groupUpdatingId === account.id
                    }
                    onClick={() => onUpdateEnabled(account, !account.enabled)}
                    title={account.enabled ? "禁用 Claude 账号调度" : "启用 Claude 账号调度"}
                  >
                    {enabledUpdatingId === account.id ? (
                      <Loader2 className={spinnerClass} size={14} />
                    ) : (
                      enabledToggleLabel(account.enabled)
                    )}
                  </button>}
                  <button
                    className={buttonSmall}
                    disabled={!canViewOverride ||
                      deletingId === account.id ||
                      enabledUpdatingId === account.id ||
                      groupUpdatingId === account.id
                    }
                    onClick={() => onOpenOverride(account)}
                    title={canViewOverride ? "查看请求覆盖" : "未获得查看账号覆盖权限"}
                  >
                    <Settings size={14} />
                    覆盖
                  </button>
                  {access.isOwner && <button
                    className={buttonSmallDanger}
                    disabled={
                      deletingId === account.id ||
                      enabledUpdatingId === account.id ||
                      groupUpdatingId === account.id
                    }
                    onClick={() => onDelete(account)}
                    title="删除 Claude 账号"
                  >
                    {deletingId === account.id ? (
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
