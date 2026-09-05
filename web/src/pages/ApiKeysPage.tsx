import { RowActions } from "../components/RowActions";
import { useState } from "react";
import { Ban, Copy, Eye, EyeOff, KeyRound, ListChecks, Loader2, Pencil, Plus, Power, Trash2 } from "lucide-react";
import { ListPager } from "../components/ListPager";
import { StatusBadge } from "../components/StatusBadge";
import { formatDateTime } from "../lib/format";
import {
  buttonPrimary,
  buttonSmall,
  cellMainClass,
  cellNoteClass,
  disabledRowClass,
  emptyStateClass,
  entryTitleClass,
  iconButtonSmall,
  panelClass,
  panelHeaderClass,
  panelTitleClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../lib/ui";
import type { ApiKey } from "../types";

interface ApiKeysPageProps {
  apiKeys: ApiKey[];
  loading: boolean;
  updatingId: string | null;
  offset: number;
  pageSize: number;
  nextOffset: number | null;
  onCreate: () => void;
  onEditModels: (apiKey: ApiKey) => void;
  onEditPlugin: (apiKey: ApiKey) => void;
  onToggleEnabled: (apiKey: ApiKey) => void;
  onDelete: (apiKey: ApiKey) => void;
  onCopy: (apiKey: ApiKey) => void;
  onPageChange: (offset: number) => void;
}

export function ApiKeysPage({
  apiKeys,
  loading,
  updatingId,
  offset,
  pageSize,
  nextOffset,
  onCreate,
  onEditModels,
  onEditPlugin,
  onToggleEnabled,
  onDelete,
  onCopy,
  onPageChange,
}: ApiKeysPageProps) {
  // 可见状态仅保存在当前页面组件中；刷新或重新进入页面后自动恢复为默认隐藏。
  const [visibleApiKeyIds, setVisibleApiKeyIds] = useState<Set<string>>(() => new Set());

  function toggleApiKeyVisibility(id: string) {
    setVisibleApiKeyIds((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  return (
    <section className="min-w-0">
      <div className={`${panelClass} overflow-hidden`}>
        <div className={panelHeaderClass}>
          <h2 className={panelTitleClass}>API Key</h2>
          <button className={buttonPrimary} onClick={onCreate}>
            <Plus size={16} />
            创建 Key
          </button>
        </div>
        {loading ? (
          <div className={emptyStateClass}>
            <Loader2 className={spinnerClass} size={24} />
            <span>正在加载 API Key</span>
          </div>
        ) : apiKeys.length === 0 ? (
          <div className={emptyStateClass}>
            <KeyRound size={24} />
            <span>还没有创建 API Key</span>
          </div>
        ) : (
          <div className={tableScrollClass}>
            <table className={`${tableClass} min-w-[96rem]`}>
              <thead>
                <tr>
                  <th>名称</th>
                  <th>Provider 分组</th>
                  <th>API Key</th>
                  <th>模型白名单</th>
                  <th>插件</th>
                  <th>状态</th>
                  <th>更新</th>
                  <th className="w-20 text-right">操作</th>
                </tr>
              </thead>
              <tbody>
                {apiKeys.map((apiKey) => (
                  <tr key={apiKey.id} className={apiKey.enabled ? undefined : disabledRowClass}>
                    <td>
                      <strong className={entryTitleClass}>{apiKey.name}</strong>
                    </td>
                    <td>
                      <div className={cellMainClass}>{apiKey.group.name}</div>
                      <p className={cellNoteClass}>
                        {apiKey.group.provider === "gpt" ? "GPT" : "Claude"}
                      </p>
                      {!apiKey.group_authorized && (
                        <p className="text-xs font-medium text-red-600 dark:text-red-400">
                          分组授权已撤销
                        </p>
                      )}
                    </td>
                    <td>
                      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-2">
                        <code
                          className="truncate font-mono text-xs text-slate-700 dark:text-slate-300"
                          aria-label={
                            visibleApiKeyIds.has(apiKey.id) ? "API Key 已显示" : "API Key 已隐藏"
                          }
                        >
                          {visibleApiKeyIds.has(apiKey.id)
                            ? apiKey.api_key
                            : "••••••••••••••••••••••••"}
                        </code>
                        <button
                          type="button"
                          className={iconButtonSmall}
                          onClick={() => toggleApiKeyVisibility(apiKey.id)}
                          aria-label={
                            visibleApiKeyIds.has(apiKey.id) ? "隐藏 API Key" : "显示 API Key"
                          }
                          aria-pressed={visibleApiKeyIds.has(apiKey.id)}
                          title={visibleApiKeyIds.has(apiKey.id) ? "隐藏 API Key" : "显示 API Key"}
                        >
                          {visibleApiKeyIds.has(apiKey.id) ? (
                            <EyeOff size={15} />
                          ) : (
                            <Eye size={15} />
                          )}
                        </button>
                        <button
                          type="button"
                          className={buttonSmall}
                          onClick={() => onCopy(apiKey)}
                          title="复制 API Key"
                        >
                          <Copy size={14} />
                          复制
                        </button>
                      </div>
                    </td>
                    <td>
                      <div
                        className="flex flex-wrap gap-1.5"
                        aria-label={`${apiKey.name} 的模型白名单`}
                      >
                        {apiKey.allowed_models.map((model) => {
                          const allowedByGroup = apiKey.group_allowed_models.includes(model);
                          return (
                            <code
                              key={model}
                              className={
                                allowedByGroup
                                  ? "max-w-full break-all rounded-md bg-indigo-50 px-2 py-1 font-mono text-[11px] leading-4 text-indigo-700 dark:bg-indigo-950/60 dark:text-indigo-300"
                                  : "max-w-full break-all rounded-md bg-amber-50 px-2 py-1 font-mono text-[11px] leading-4 text-amber-700 line-through dark:bg-amber-950/60 dark:text-amber-300"
                              }
                              title={allowedByGroup ? model : `${model}（当前分组未授权）`}
                            >
                              {model}
                            </code>
                          );
                        })}
                      </div>
                    </td>
                    <td>
                      {apiKey.plugin ? (
                        <div>
                          <div className={cellMainClass}>{apiKey.plugin.suite_name}</div>
                          <p className={cellNoteClass}>
                            v{apiKey.plugin.version} · {apiKey.plugin.provider.toUpperCase()}
                            {!apiKey.plugin.suite_enabled && " · 已停用"}
                          </p>
                        </div>
                      ) : (
                        <span className={cellNoteClass}>未绑定</span>
                      )}
                    </td>
                    <td>
                      <StatusBadge status={apiKey.enabled ? "active" : "disabled"} />
                      {apiKey.disabled_at && (
                        <p className={cellNoteClass}>禁用：{formatDateTime(apiKey.disabled_at)}</p>
                      )}
                    </td>
                    <td>
                      <div className={cellMainClass}>{formatDateTime(apiKey.updated_at)}</div>
                      <p className={cellNoteClass}>创建：{formatDateTime(apiKey.created_at)}</p>
                    </td>
                    <td>
                      <RowActions
                        resourceLabel={apiKey.name}
                        busy={updatingId === apiKey.id}
                        actions={[
                          {
                            id: "edit-models", label: "修改模型", icon: ListChecks,
                            disabled: !apiKey.group_authorized, opensDialog: true,
                            description: apiKey.group_authorized ? "修改模型白名单" : "分组授权已撤销",
                            onSelect: () => onEditModels(apiKey),
                          },
                          {
                            id: "edit-plugin", label: "修改插件", icon: Pencil,
                            disabled: !apiKey.group_authorized, opensDialog: true,
                            description: apiKey.group_authorized ? "修改插件绑定" : "分组授权已撤销",
                            onSelect: () => onEditPlugin(apiKey),
                          },
                          {
                            id: "toggle-enabled", label: apiKey.enabled ? "禁用" : "启用",
                            icon: apiKey.enabled ? Ban : Power, disabled: !apiKey.group_authorized,
                            onSelect: () => onToggleEnabled(apiKey),
                          },
                          {
                            id: "delete", label: "删除 API Key", icon: Trash2,
                            danger: true, opensDialog: true, onSelect: () => onDelete(apiKey),
                          },
                        ]}
                      />
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
          itemCount={apiKeys.length}
          nextOffset={nextOffset}
          loading={loading}
          label="个 Key"
          onPageChange={onPageChange}
        />
      </div>
    </section>
  );
}
