import { Archive, FolderTree, ListChecks, Loader2, Pencil, RotateCcw, Save, X } from "lucide-react";
import { useState } from "react";
import type { FormEvent } from "react";
import { StatusBadge } from "../../components/StatusBadge";
import { formatDateTime } from "../../lib/format";
import {
  buttonSmall,
  buttonSmallDanger,
  cellMainClass,
  cellNoteClass,
  disabledRowClass,
  emptyStateClass,
  inputClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../../lib/ui";
import type { ProviderGroupSummary } from "../../types";

interface ProviderGroupsTableProps {
  groups: ProviderGroupSummary[];
  savingId: string | null;
  onRename: (group: ProviderGroupSummary, name: string) => Promise<boolean>;
  onEditModels: (group: ProviderGroupSummary) => void;
  onToggleEnabled: (group: ProviderGroupSummary) => Promise<boolean>;
}

/**
 * 当前 Provider 的分组主视图。
 *
 * 新建分组由独立弹窗负责；表格只承担现有分组的展示、重命名与停用/恢复，避免一个弹窗
 * 同时承载创建、列表和行级编辑三种职责。
 */
export function ProviderGroupsTable({
  groups,
  savingId,
  onRename,
  onEditModels,
  onToggleEnabled,
}: ProviderGroupsTableProps) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");

  async function submitRename(event: FormEvent<HTMLFormElement>, group: ProviderGroupSummary) {
    event.preventDefault();
    const name = editingName.trim();
    if (!name || savingId) {
      return;
    }
    if (await onRename(group, name)) {
      setEditingId(null);
      setEditingName("");
    }
  }

  if (groups.length === 0) {
    return (
      <div className={emptyStateClass}>
        <FolderTree size={24} />
        <span>当前 Provider 还没有分组</span>
      </div>
    );
  }

  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[82rem]`}>
        <thead>
          <tr>
            <th>名称</th>
            <th>限制模型</th>
            <th>关联资源</th>
            <th>更新时间</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {groups.map((group) => {
            const editing = editingId === group.id;
            const saving = savingId === group.id;
            return (
              <tr key={group.id} className={group.enabled ? undefined : disabledRowClass}>
                <td>
                  {editing ? (
                    <form
                      className="grid min-w-64 gap-2"
                      onSubmit={(event) => submitRename(event, group)}
                    >
                      <input
                        className={inputClass}
                        value={editingName}
                        onChange={(event) => setEditingName(event.target.value)}
                        maxLength={128}
                        aria-label={`重命名分组 ${group.name}`}
                        autoFocus
                      />
                      <div className="flex gap-2">
                        <button
                          className={buttonSmall}
                          disabled={!editingName.trim() || saving}
                          title="保存分组名称"
                        >
                          {saving ? <Loader2 className={spinnerClass} size={14} /> : <Save size={14} />}
                          保存
                        </button>
                        <button
                          type="button"
                          className={buttonSmall}
                          disabled={saving}
                          onClick={() => {
                            setEditingId(null);
                            setEditingName("");
                          }}
                        >
                          <X size={14} />
                          取消
                        </button>
                      </div>
                    </form>
                  ) : (
                    <div className="grid gap-2">
                      <strong className="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
                        {group.name}
                      </strong>
                      <StatusBadge status={group.enabled ? "active" : "disabled"} />
                    </div>
                  )}
                </td>
                <td>
                  <div className="flex flex-wrap gap-1.5">
                    {group.allowed_models.map((model) => (
                      <code
                        key={model}
                        className="rounded-md bg-indigo-50 px-2 py-1 font-mono text-[11px] text-indigo-700 dark:bg-indigo-950/60 dark:text-indigo-300"
                      >
                        {model}
                      </code>
                    ))}
                  </div>
                </td>
                <td>
                  <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                    <span>账号 {group.counts.account_count}</span>
                    <span>官方 Key {group.counts.upstream_api_key_count}</span>
                    <span>启用网关 Key {group.counts.enabled_gateway_api_key_count}</span>
                  </div>
                </td>
                <td>
                  <div className={cellMainClass}>{formatDateTime(group.updated_at)}</div>
                  <p className={cellNoteClass}>创建：{formatDateTime(group.created_at)}</p>
                </td>
                <td>
                  {!editing && (
                    <div className="grid max-w-64 grid-cols-2 gap-2">
                      <button
                        className={buttonSmall}
                        disabled={Boolean(savingId)}
                        onClick={() => {
                          setEditingId(group.id);
                          setEditingName(group.name);
                        }}
                      >
                        <Pencil size={14} />
                        重命名
                      </button>
                      <button
                        className={buttonSmall}
                        disabled={Boolean(savingId)}
                        onClick={() => onEditModels(group)}
                      >
                        <ListChecks size={14} />
                        模型
                      </button>
                      <button
                        className={group.enabled ? buttonSmallDanger : buttonSmall}
                        disabled={Boolean(savingId)}
                        onClick={() => void onToggleEnabled(group)}
                      >
                        {saving ? (
                          <Loader2 className={spinnerClass} size={14} />
                        ) : group.enabled ? (
                          <Archive size={14} />
                        ) : (
                          <RotateCcw size={14} />
                        )}
                        {group.enabled ? "停用" : "恢复"}
                      </button>
                    </div>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
