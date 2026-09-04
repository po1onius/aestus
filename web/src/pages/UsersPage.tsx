import { Activity, Gauge, Loader2, Plus, ShieldCheck, UserCog } from "lucide-react";
import { ListPager } from "../components/ListPager";
import { StatusBadge } from "../components/StatusBadge";
import { formatDateTime } from "../lib/format";
import {
  actionStackClass,
  buttonPrimary,
  buttonSmall,
  cellMainClass,
  cellNoteClass,
  disabledRowClass,
  emptyStateClass,
  entryStackClass,
  entryTitleClass,
  panelClass,
  panelHeaderClass,
  panelTitleClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../lib/ui";
import type { DashboardUser, DashboardUserListItem } from "../types";

interface UsersPageProps {
  users: DashboardUserListItem[];
  loading: boolean;
  updatingId: string | null;
  currentUserId: string;
  offset: number;
  pageSize: number;
  nextOffset: number | null;
  onAdd: () => void;
  onOpenQuota: (user: DashboardUser) => void;
  onOpenConcurrency: (user: DashboardUser) => void;
  onOpenGroupGrants: (user: DashboardUser) => void;
  onToggleStatus: (user: DashboardUser) => void;
  onPageChange: (offset: number) => void;
}

export function UsersPage({
  users,
  loading,
  updatingId,
  currentUserId,
  offset,
  pageSize,
  nextOffset,
  onAdd,
  onOpenQuota,
  onOpenConcurrency,
  onOpenGroupGrants,
  onToggleStatus,
  onPageChange,
}: UsersPageProps) {
  return (
    <section className="min-w-0">
      <div className={`${panelClass} overflow-hidden`}>
        <div className={panelHeaderClass}>
          <h2 className={panelTitleClass}>用户</h2>
          <button className={buttonPrimary} onClick={onAdd}>
            <Plus size={16} />
            添加用户
          </button>
        </div>
        {loading ? (
          <div className={emptyStateClass}>
            <Loader2 className={spinnerClass} size={24} />
            <span>正在加载用户</span>
          </div>
        ) : users.length === 0 ? (
          <div className={emptyStateClass}>
            <UserCog size={24} />
            <span>还没有用户</span>
          </div>
        ) : (
          <div className={tableScrollClass}>
            <table className={`${tableClass} min-w-[72rem]`}>
              <thead>
                <tr>
                  <th>用户</th>
                  <th>角色</th>
                  <th>Token 额度</th>
                  <th>Provider 并发</th>
                  <th>状态</th>
                  <th>更新</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {users.map((user) => (
                  <tr key={user.id} className={user.enabled ? undefined : disabledRowClass}>
                    <td>
                      <div className={entryStackClass}>
                        <strong className={entryTitleClass}>{user.username}</strong>
                        <span className={cellNoteClass}>{user.email}</span>
                      </div>
                    </td>
                    <td>
                      <div className={cellMainClass}>
                        {user.role === "tenant_owner" ? "租户 owner" : "普通用户"}
                      </div>
                    </td>
                    <td>
                      <div className={cellMainClass}>{user.quota}</div>
                    </td>
                    <td>
                      <div className={entryStackClass}>
                        <span className={cellMainClass}>
                          GPT：{user.current_concurrency.gpt} / {user.max_concurrency ?? "不限"}
                        </span>
                        <span className={cellNoteClass}>
                          Claude：{user.current_concurrency.claude} / {user.max_concurrency ?? "不限"}
                        </span>
                      </div>
                    </td>
                    <td>
                      <StatusBadge status={user.enabled ? "active" : "disabled"} />
                      {user.disabled_at && (
                        <p className={cellNoteClass}>禁用：{formatDateTime(user.disabled_at)}</p>
                      )}
                    </td>
                    <td>
                      <div className={cellMainClass}>{formatDateTime(user.updated_at)}</div>
                      <p className={cellNoteClass}>创建：{formatDateTime(user.created_at)}</p>
                    </td>
                    <td>
                      <div className={actionStackClass}>
                        <button
                          className={buttonSmall}
                          disabled={updatingId === user.id || user.role !== "tenant_user"}
                          onClick={() => onOpenGroupGrants(user)}
                        >
                          <ShieldCheck size={14} />
                          分组授权
                        </button>
                        <button
                          className={buttonSmall}
                          disabled={updatingId === user.id}
                          onClick={() => onOpenQuota(user)}
                        >
                          {updatingId === user.id ? (
                            <Loader2 className={spinnerClass} size={14} />
                          ) : (
                            <Gauge size={14} />
                          )}
                          改额度
                        </button>
                        <button
                          className={buttonSmall}
                          disabled={updatingId === user.id}
                          onClick={() => onOpenConcurrency(user)}
                        >
                          {updatingId === user.id ? (
                            <Loader2 className={spinnerClass} size={14} />
                          ) : (
                            <Activity size={14} />
                          )}
                          改并发
                        </button>
                        <button
                          className={buttonSmall}
                          disabled={
                            updatingId === user.id ||
                            user.id === currentUserId ||
                            user.role !== "tenant_user"
                          }
                          onClick={() => onToggleStatus(user)}
                        >
                          {user.enabled ? "禁用" : "启用"}
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <ListPager
          offset={offset}
          limit={pageSize}
          itemCount={users.length}
          nextOffset={nextOffset}
          loading={loading}
          label="个用户"
          onPageChange={onPageChange}
        />
      </div>
    </section>
  );
}
