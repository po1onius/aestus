import { useMemo } from "react";
import { FileUp, Loader2, PlugZap, Plus, Power, Trash2 } from "lucide-react";
import { StatusBadge } from "../components/StatusBadge";
import { formatDateTime } from "../lib/format";
import {
  buttonPrimary,
  buttonSmall,
  buttonSmallDanger,
  cellMainClass,
  cellNoteClass,
  disabledRowClass,
  emptyStateClass,
  panelClass,
  panelDescriptionClass,
  panelHeaderClass,
  panelTitleClass,
  spinnerClass,
  tableClass,
  tableScrollClass,
} from "../lib/ui";
import type {
  PluginArtifactSummary,
  PluginReleaseSummary,
  PluginSlot,
} from "../types";

interface PluginsPageProps {
  plugins: PluginReleaseSummary[];
  loading: boolean;
  savingId: string | null;
  onAdd: () => void;
  onOpenPublish: (plugin: PluginReleaseSummary) => void;
  onToggleEnabled: (plugin: PluginReleaseSummary) => Promise<void>;
  onDelete: (plugin: PluginReleaseSummary) => void;
}

const slotLabels: Record<PluginSlot, string> = {
  request: "请求",
  buffered_response: "非流式响应",
  stream_response: "流式 SSE",
};

/**
 * 管理端以完整套件 release 为发布和绑定单位。每次发布都重新声明三个可空插槽，未上传
 * 的插槽不会继承旧版本，而是明确回退到 Provider 原生流程。
 */
export function PluginsPage({
  plugins,
  loading,
  savingId,
  onAdd,
  onOpenPublish,
  onToggleEnabled,
  onDelete,
}: PluginsPageProps) {
  const latestReleaseIds = useMemo(() => {
    const latest = new Map<string, PluginReleaseSummary>();
    for (const release of plugins) {
      const current = latest.get(release.suite_id);
      if (!current || release.version > current.version) latest.set(release.suite_id, release);
    }
    return new Set([...latest.values()].map((release) => release.id));
  }, [plugins]);

  return (
    <section className="min-w-0">
      <div className={`${panelClass} overflow-hidden`}>
        <div className={panelHeaderClass}>
          <div>
            <h2 className={panelTitleClass}>插件列表</h2>
          </div>
          <button className={buttonPrimary} disabled={savingId !== null} onClick={onAdd}>
            <Plus size={16} />
            添加
          </button>
        </div>
        {loading ? (
          <div className={emptyStateClass}>
            <Loader2 className={spinnerClass} size={24} />
            <span>正在加载插件</span>
          </div>
        ) : plugins.length === 0 ? (
          <div className={emptyStateClass}>
            <PlugZap size={24} />
            <span>还没有添加插件</span>
          </div>
        ) : (
          <div className={tableScrollClass}>
            <table className={`${tableClass} min-w-[96rem]`}>
              <thead>
                <tr>
                  <th>插件</th>
                  <th>Provider</th>
                  <th>版本</th>
                  <th>Artifact 插槽</th>
                  <th>Manifest</th>
                  <th>发布于</th>
                  <th>状态</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {plugins.map((plugin) => {
                  const isLatest = latestReleaseIds.has(plugin.id);
                  return (
                    <tr
                      key={plugin.id}
                      className={plugin.suite_enabled ? undefined : disabledRowClass}
                    >
                      <td>
                        <div className={cellMainClass}>{plugin.suite_name}</div>
                        <p className={cellNoteClass} title={plugin.description}>
                          {plugin.description || "无描述"}
                        </p>
                      </td>
                      <td>{plugin.provider === "gpt" ? "GPT · Responses" : "Claude · Messages"}</td>
                      <td>
                        <div className={cellMainClass}>v{plugin.version}</div>
                        {isLatest && <p className={cellNoteClass}>最新版本</p>}
                      </td>
                      <td>
                        <div className="grid gap-1.5">
                          {plugin.artifacts.map((artifact) => (
                            <ArtifactBadge key={artifact.id} artifact={artifact} />
                          ))}
                        </div>
                      </td>
                      <td>
                        <code
                          className="block max-w-48 truncate font-mono text-xs"
                          title={plugin.manifest_sha256}
                        >
                          {plugin.manifest_sha256.slice(0, 16)}…
                        </code>
                      </td>
                      <td>{formatDateTime(plugin.published_at)}</td>
                      <td><StatusBadge status={plugin.suite_enabled ? "active" : "disabled"} /></td>
                      <td>
                        {isLatest ? (
                          <div className="grid min-w-36 gap-2">
                            <button
                              className={buttonSmall}
                              disabled={savingId !== null}
                              onClick={() => onOpenPublish(plugin)}
                            >
                              <FileUp size={14} />
                              发布新版本
                            </button>
                            <button
                              className={buttonSmall}
                              disabled={savingId !== null}
                              onClick={() => void onToggleEnabled(plugin)}
                            >
                              {savingId === plugin.suite_id ? (
                                <Loader2 className={spinnerClass} size={14} />
                              ) : (
                                <Power size={14} />
                              )}
                              {plugin.suite_enabled ? "停用插件" : "启用插件"}
                            </button>
                            <button
                              className={buttonSmallDanger}
                              disabled={savingId !== null}
                              onClick={() => onDelete(plugin)}
                              title="删除插件套件"
                            >
                              {savingId === plugin.suite_id ? (
                                <Loader2 className={spinnerClass} size={14} />
                              ) : (
                                <Trash2 size={14} />
                              )}
                              删除插件
                            </button>
                          </div>
                        ) : (
                          <span className={cellNoteClass}>历史版本</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </section>
  );
}

function ArtifactBadge({ artifact }: { artifact: PluginArtifactSummary }) {
  return (
    <div
      className="rounded-md bg-slate-100 px-2 py-1 text-xs dark:bg-slate-800"
      title={`${artifact.wasm_sha256} · ${formatBytes(artifact.wasm_size)}`}
    >
      <span className="font-medium">{slotLabels[artifact.slot]}</span>
      <span className="ml-1 text-slate-500 dark:text-slate-400">ABI {artifact.abi_version}</span>
    </div>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
