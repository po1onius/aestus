import { useState } from "react";
import { Loader2, PlugZap, Plus, Power, Trash2 } from "lucide-react";
import { RowActions } from "../components/RowActions";
import { SlidingTabList } from "../components/SlidingTabList";
import { StatusBadge } from "../components/StatusBadge";
import { canManagePlugin, pluginSourceLabel } from "../features/plugins/access";
import { pluginSlotLabels, suiteSlotFields } from "../features/plugins/slots";
import { formatDateTime } from "../lib/format";
import {
  buttonPrimary, cellMainClass, cellNoteClass, disabledRowClass, emptyStateClass,
  inputClass, panelClass, panelHeaderClass, panelTitleClass, spinnerClass,
  tableClass, tableScrollClass, tabClass, tabSelectedClass, tabIdleClass, cx,
} from "../lib/ui";
import type { DashboardUser, PluginSummary, PluginSuiteSummary, UpstreamApiKeyProvider } from "../types";

interface Props {
  user: DashboardUser;
  plugins: PluginSummary[];
  suites: PluginSuiteSummary[];
  loading: boolean;
  savingId: string | null;
  onAddPlugin: () => void;
  onAddSuite: () => void;
  onToggleEnabled: (suite: PluginSuiteSummary) => Promise<void>;
  onDeletePlugin: (plugin: PluginSummary) => void;
  onDeleteSuite: (suite: PluginSuiteSummary) => void;
}

export function PluginsPage(props: Props) {
  const [tab, setTab] = useState<"plugins" | "suites">("plugins");
  const [provider, setProvider] = useState<UpstreamApiKeyProvider>("gpt");
  const plugins = props.plugins.filter((p) => p.provider === provider);
  const suites = props.suites.filter((s) => s.provider === provider);
  const pluginById = new Map(props.plugins.map((p) => [p.id, p]));
  const empty = tab === "plugins" ? plugins.length === 0 : suites.length === 0;

  return (
    <section className="min-w-0 grid gap-4">
      <div className="flex flex-wrap items-center gap-4">
        <SlidingTabList count={2} selectedIndex={tab === "plugins" ? 0 : 1} ariaLabel="插件管理" role="group">
          {(["plugins", "suites"] as const).map((value) => (
            <button key={value} type="button" aria-pressed={tab === value} className={cx(tabClass, tab === value ? tabSelectedClass : tabIdleClass)} onClick={() => setTab(value)}>
              {value === "plugins" ? "WASM 插件" : "套件"}
            </button>
          ))}
        </SlidingTabList>
        <select aria-label="插件 Provider" className={`${inputClass} max-w-56`} value={provider} onChange={(e) => setProvider(e.target.value as UpstreamApiKeyProvider)}>
          <option value="gpt">GPT · Responses</option>
          <option value="claude">Claude · Messages</option>
        </select>
      </div>
      <p className={cellNoteClass}>{props.user.role === "platform_admin"
        ? "平台公共插件和套件对所有租户可用。公共套件只能选择公共插件。"
        : "可以使用平台公共资源，管理本租户资源；私有套件可组合公共插件与本租户插件。"}</p>
      <div className={`${panelClass} overflow-hidden`}>
        <div className={panelHeaderClass}>
          <h2 className={panelTitleClass}>{tab === "plugins" ? "WASM 插件" : "套件"}</h2>
          <button className={buttonPrimary} disabled={props.savingId !== null} onClick={tab === "plugins" ? props.onAddPlugin : props.onAddSuite}>
            <Plus size={16} />{tab === "plugins" ? "上传插件" : "创建套件"}
          </button>
        </div>
        {props.loading ? (
          <div className={emptyStateClass}><Loader2 className={spinnerClass} size={24} /><span>正在加载</span></div>
        ) : empty ? (
          <div className={emptyStateClass}><PlugZap size={24} /><span>{tab === "plugins" ? "该 Provider 还没有插件，请先按插槽上传" : "该 Provider 还没有套件，请从已有插件中创建组合"}</span></div>
        ) : (
          <div className={tableScrollClass}>
            {tab === "plugins" ? (
              <table className={`${tableClass} min-w-[64rem]`}>
                <thead><tr><th>名称</th><th>来源</th><th>类型（插槽）</th><th>WASM 大小</th><th>可见引用套件</th><th>上传时间</th><th className="w-24 text-right">操作</th></tr></thead>
                <tbody>{plugins.map((plugin) => (
                  <tr key={plugin.id}>
                    <td><div className={cellMainClass}>{plugin.name}</div><p className={cellNoteClass} title={plugin.description}>{plugin.description || "无备注"}</p></td>
                    <td>{pluginSourceLabel(plugin.tenant_id)}</td>
                    <td>{pluginSlotLabels[plugin.slot]}</td>
                    <td>{formatBytes(plugin.wasm_size)}</td>
                    <td>{props.suites.filter((s) => suiteSlotFields.some(({ field }) => s[field] === plugin.id)).length}</td>
                    <td>{formatDateTime(plugin.created_at)}</td>
                    <td>{canManagePlugin(props.user, plugin.tenant_id) ? <RowActions resourceLabel={plugin.name} busy={props.savingId === plugin.id} actions={[
                      { id: "delete", label: "删除插件", icon: Trash2, disabled: props.savingId !== null, danger: true, opensDialog: true, onSelect: () => props.onDeletePlugin(plugin) },
                    ]} /> : <span className={cellNoteClass}>只读</span>}</td>
                  </tr>
                ))}</tbody>
              </table>
            ) : (
              <table className={`${tableClass} min-w-[76rem]`}>
                <thead><tr><th>名称</th><th>来源</th>{suiteSlotFields.map(({ slot }) => <th key={slot}>{pluginSlotLabels[slot]}</th>)}<th>创建时间</th><th>状态</th><th className="w-24 text-right">操作</th></tr></thead>
                <tbody>{suites.map((suite) => (
                  <tr key={suite.id} className={suite.enabled ? undefined : disabledRowClass}>
                    <td><div className={cellMainClass}>{suite.name}</div><p className={cellNoteClass} title={suite.description}>{suite.description || "无备注"}</p></td>
                    <td>{pluginSourceLabel(suite.tenant_id)}</td>
                    {suiteSlotFields.map(({ field }) => <td key={field}>{suite[field] ? (pluginById.get(suite[field])?.name ?? "插件不存在") : <span className={cellNoteClass}>原生处理</span>}</td>)}
                    <td>{formatDateTime(suite.created_at)}</td>
                    <td><StatusBadge status={suite.enabled ? "active" : "disabled"} /></td>
                    <td>{canManagePlugin(props.user, suite.tenant_id) ? <RowActions resourceLabel={suite.name} busy={props.savingId === suite.id} actions={[
                      { id: "toggle", label: suite.enabled ? "停用套件" : "启用套件", icon: Power, disabled: props.savingId !== null, onSelect: () => void props.onToggleEnabled(suite) },
                      { id: "delete", label: "删除套件", icon: Trash2, disabled: props.savingId !== null, danger: true, opensDialog: true, onSelect: () => props.onDeleteSuite(suite) },
                    ]} /> : <span className={cellNoteClass}>只读</span>}</td>
                  </tr>
                ))}</tbody>
              </table>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
