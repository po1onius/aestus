import { Loader2, Search, Settings, Trash2 } from "lucide-react";
import { RuntimeBadge } from "../../components/RuntimeBadge";
import { StatusBadge } from "../../components/StatusBadge";
import { formatDateTime, formatOptionalDateTime } from "../../lib/format";
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
import type { GptAccount, GptAccountQuotaResponse, ProviderGroup } from "../../types";
import { ProviderGroupCell } from "./ProviderGroupCell";
import { QuotaCell } from "./QuotaCell";
import { enabledToggleLabel } from "./utils";

interface GptAccountsTableProps {
  accounts: GptAccount[];
  quotas: Record<string, GptAccountQuotaResponse>;
  quotaRefreshingIds: Record<string, boolean>;
  groups: ProviderGroup[];
  groupUpdatingId: string | null;
  enabledUpdatingId: string | null;
  deletingId: string | null;
  onRefreshQuota: (account: GptAccount) => void;
  onUpdateGroup: (account: GptAccount, groupId: string) => void;
  onUpdateEnabled: (account: GptAccount, enabled: boolean) => void;
  onOpenOverride: (account: GptAccount) => void;
  onDelete: (account: GptAccount) => void;
}

/** GPT OAuth 账号表格。所有写操作均由上层控制器执行，组件本身保持无副作用。 */
export function GptAccountsTable({
  accounts,
  quotas,
  quotaRefreshingIds,
  groups,
  groupUpdatingId,
  enabledUpdatingId,
  deletingId,
  onRefreshQuota,
  onUpdateGroup,
  onUpdateEnabled,
  onOpenOverride,
  onDelete,
}: GptAccountsTableProps) {
  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[100rem]`}>
        <thead>
          <tr>
            <th>账号</th>
            <th>所在组</th>
            <th>计划</th>
            <th>额度</th>
            <th>状态</th>
            <th>调度</th>
            <th>更新</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {accounts.map((account) => {
            const busy =
              Boolean(quotaRefreshingIds[account.id]) ||
              deletingId === account.id ||
              groupUpdatingId === account.id ||
              enabledUpdatingId === account.id;

            return (
              <tr key={account.id} className={account.enabled ? undefined : disabledRowClass}>
                <td>
                  <div className={entryStackClass}>
                    <strong className={entryTitleClass} title={account.email || "未命名账号"}>{account.email || "未命名账号"}</strong>
                    <span className={cellNoteClass} title={account.account_id || "未设置 chatgpt_account_id"}>
                      {account.account_id || "未设置 chatgpt_account_id"}
                    </span>
                    <span className={cellNoteClass} title={`client_id: ${account.client_id}`}>client_id: {account.client_id}</span>
                  </div>
                </td>
                <td>
                <ProviderGroupCell
                  resourceLabel={account.email || account.id}
                  provider="gpt"
                  group={account.group}
                    groups={groups}
                    disabled={busy}
                    onChange={(groupId) => onUpdateGroup(account, groupId)}
                  />
                </td>
                <td>
                  <div className={cellMainClass}>{account.plan_type}</div>
                </td>
                <td>
                  <QuotaCell quota={quotas[account.id]} />
                </td>
                <td>
                  <div className="grid gap-1.5">
                    <StatusBadge status={account.status} />
                    <p className={cellNoteClass}>管理员：{account.enabled ? "已启用" : "已禁用"}</p>
                    {account.status_reason && (
                      <p className={`${cellNoteClass} max-w-40 text-red-700 dark:text-red-400`} title={account.status_reason}>
                        {account.status_reason}
                      </p>
                    )}
                  </div>
                </td>
                <td>
                  <RuntimeBadge runtime={account.runtime} />
                  <p className={cellNoteClass}>
                    并发：{account.runtime.inflight_count} ·{" "}
                    {account.runtime.runtime_ready ? "runtime 就绪" : "runtime 未就绪"}
                  </p>
                  <p className={cellNoteClass}>Token：{account.runtime.token_usable ? "可用" : "不可用"}</p>
                  <p className={cellNoteClass}>
                    下次刷新：{formatOptionalDateTime(account.runtime.next_token_refresh_at)}
                  </p>
                  {(account.runtime.quota_resets_at || account.quota_resets_at) && (
                    <p className={cellNoteClass}>
                      额度恢复：
                      {formatOptionalDateTime(account.runtime.quota_resets_at || account.quota_resets_at)}
                    </p>
                  )}
                </td>
                <td>
                  <div className={cellMainClass}>{formatDateTime(account.updated_at)}</div>
                  <p className={cellNoteClass}>更新：{formatDateTime(account.updated_at)}</p>
                  <p className={cellNoteClass}>运行态：{account.runtime.runtime_exists ? "已发布" : "未发布"}</p>
                </td>
                <td>
                  <div className={actionStackClass}>
                    <button
                      className={buttonSmall}
                      disabled={busy}
                      onClick={() => onRefreshQuota(account)}
                      title="查询账号额度"
                    >
                      {quotaRefreshingIds[account.id] ? (
                        <Loader2 className={spinnerClass} size={14} />
                      ) : (
                        <Search size={14} />
                      )}
                      查询额度
                    </button>
                    <button
                      className={buttonSmall}
                      disabled={busy}
                      onClick={() => onUpdateEnabled(account, !account.enabled)}
                      title={account.enabled ? "禁用账号调度" : "启用账号调度"}
                    >
                      {enabledUpdatingId === account.id ? (
                        <Loader2 className={spinnerClass} size={14} />
                      ) : (
                        enabledToggleLabel(account.enabled)
                      )}
                    </button>
                    <button
                      className={buttonSmall}
                      disabled={busy}
                      onClick={() => onOpenOverride(account)}
                      title="请求覆盖"
                    >
                      <Settings size={14} />
                      覆盖
                    </button>
                    <button
                      className={buttonSmallDanger}
                      disabled={busy}
                      onClick={() => onDelete(account)}
                      title="删除账号"
                    >
                      {deletingId === account.id ? (
                        <Loader2 className={spinnerClass} size={14} />
                      ) : (
                        <Trash2 size={14} />
                      )}
                      删除
                    </button>
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
