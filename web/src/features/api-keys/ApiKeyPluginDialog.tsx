import { pluginSourceLabel } from "../plugins/access";
import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  spinnerClass,
} from "../../lib/ui";
import type { ApiKey, PluginSuiteSummary } from "../../types";

interface ApiKeyPluginDialogProps {
  apiKey: ApiKey;
  plugins: PluginSuiteSummary[];
  pluginSuiteId: string;
  saving: boolean;
  onPluginChange: (suiteId: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

/** 只修改网关 Key 的套件绑定，不混入名称、分组或模型白名单编辑状态。 */
export function ApiKeyPluginDialog(props: ApiKeyPluginDialogProps) {
  const compatiblePlugins = props.plugins.filter(
    (plugin) => plugin.provider === props.apiKey.group?.provider && plugin.enabled,
  );
  const currentPlugin = props.apiKey.plugin;
  const currentPluginIsSelectable =
    currentPlugin !== null && compatiblePlugins.some((plugin) => plugin.id === currentPlugin.id);
  const originalSuiteId = props.apiKey.plugin_suite_id ?? "";

  return (
    <Modal
      titleId="apiKeyPluginTitle"
      title="修改套件绑定"
      description={`为 API Key“${props.apiKey.name}”选择插件套件。`}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={props.onSubmit}>
        <div className="rounded-lg border border-slate-200 bg-slate-50 px-3 py-2.5 text-sm dark:border-slate-700 dark:bg-slate-950/60">
          <div className="font-medium text-slate-800 dark:text-slate-200">
            {props.apiKey.group?.name ?? "分组已删除，Key 无效"}
          </div>
          <div className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
            Provider：{props.apiKey.group?.provider.toUpperCase()}
          </div>
        </div>

        <label className={fieldStack}>
          <span className={fieldLabel}>插件套件</span>
          <select
            className={inputClass}
            value={props.pluginSuiteId}
            disabled={props.saving}
            onChange={(event) => props.onPluginChange(event.target.value)}
          >
            <option value="">不使用插件</option>
            {!currentPlugin && props.apiKey.plugin_suite_id && (
              <option value={props.apiKey.plugin_suite_id} disabled>原套件已删除（绑定失效）</option>
            )}
            {currentPlugin && !currentPluginIsSelectable && (
              <option value={currentPlugin.id} disabled>
                {currentPlugin.name} · {pluginSourceLabel(currentPlugin.tenant_id)}（已停用）
              </option>
            )}
            {compatiblePlugins.map((plugin) => (
              <option key={plugin.id} value={plugin.id}>
                {plugin.name} · {pluginSourceLabel(plugin.tenant_id)}
              </option>
            ))}
          </select>
          <span className={fieldHelp}>
            只列出与当前 Provider 匹配的启用套件；选择“不使用插件”会解除现有绑定。插件仅作用于 OAuth 账号的 Responses / Messages 请求。
          </span>
        </label>

        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={props.saving || !props.apiKey.group || props.pluginSuiteId === originalSuiteId}
        >
          {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
          保存插件绑定
        </button>
      </form>
    </Modal>
  );
}
