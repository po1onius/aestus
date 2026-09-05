import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { ProviderGroupPicker } from "../../components/ProviderGroupPicker";
import {
  buttonPrimary,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
} from "../../lib/ui";
import type { PluginReleaseSummary, ProviderGroup } from "../../types";

interface ApiKeyCreateDialogProps {
  name: string;
  selectedModels: string[];
  saving: boolean;
  groups: ProviderGroup[];
  groupId: string;
  plugins: PluginReleaseSummary[];
  pluginReleaseId: string;
  onNameChange: (value: string) => void;
  onModelsChange: (models: string[]) => void;
  onGroupChange: (groupId: string) => void;
  onPluginChange: (releaseId: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

export function ApiKeyCreateDialog(props: ApiKeyCreateDialogProps) {
  const selectedGroup = props.groups.find((group) => group.id === props.groupId) ?? null;
  const selectedModelSet = new Set(props.selectedModels);
  const compatiblePlugins = selectedGroup
    ? props.plugins.filter(
        (plugin) => plugin.provider === selectedGroup.provider && plugin.suite_enabled,
      )
    : [];

  function toggleModel(model: string) {
    props.onModelsChange(
      selectedModelSet.has(model)
        ? props.selectedModels.filter((item) => item !== model)
        : [...props.selectedModels, model],
    );
  }

  return (
    <Modal
      titleId="apiKeyCreateTitle"
      title="创建 API Key"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-4" onSubmit={props.onSubmit}>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              名称<span className={requiredMark}>*</span>
            </span>
            <input
              className={inputClass}
              value={props.name}
              onChange={(event) => props.onNameChange(event.target.value)}
              placeholder="例如 production"
              autoComplete="off"
              maxLength={128}
              required
            />
          </label>
          <div className={fieldStack}>
            <span className={fieldLabel}>Provider 分组<span className={requiredMark}>*</span></span>
            <ProviderGroupPicker
              groups={props.groups}
              value={props.groupId}
              disabled={props.saving}
              onChange={props.onGroupChange}
            />
          </div>
          <fieldset className={fieldStack} disabled={props.saving || !selectedGroup}>
            <legend className={fieldLabel}>
              模型白名单<span className={requiredMark}>*</span>
            </legend>
            <div className="grid max-h-64 gap-2 overflow-y-auto rounded-xl border border-slate-200 p-3 dark:border-slate-700">
              {selectedGroup ? (
                selectedGroup.allowed_models.map((model) => (
                  <label
                    key={model}
                    className="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-2 text-sm text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800/70"
                  >
                    <input
                      type="checkbox"
                      className="accent-indigo-600"
                      checked={selectedModelSet.has(model)}
                      onChange={() => toggleModel(model)}
                    />
                    <code className="font-mono text-xs">{model}</code>
                  </label>
                ))
              ) : (
                <span className="px-2 py-3 text-sm text-slate-500 dark:text-slate-400">
                  请先选择 Provider 分组
                </span>
              )}
            </div>
            {selectedGroup && (
              <span className="text-xs text-slate-500 dark:text-slate-400">
                已选择 {props.selectedModels.length}/{selectedGroup.allowed_models.length} 个模型
              </span>
            )}
          </fieldset>
          <label className={fieldStack}>
            <span className={fieldLabel}>插件套件</span>
            <select
              className={inputClass}
              value={props.pluginReleaseId}
              disabled={props.saving || !selectedGroup}
              onChange={(event) => props.onPluginChange(event.target.value)}
            >
              <option value="">不使用插件</option>
              {compatiblePlugins.map((plugin) => (
                <option key={plugin.id} value={plugin.id}>
                  {plugin.suite_name} · v{plugin.version}
                </option>
              ))}
            </select>
            <span className="text-xs text-slate-500 dark:text-slate-400">
              套件只作用于调度到 OAuth 账号的请求；官方 API Key 始终使用原生流程。空插槽沿用原生流程，绑定版本保持不变。
            </span>
          </label>
          <button
            className={`${buttonPrimary} mt-1 w-full`}
            disabled={
              props.saving ||
              !props.groupId ||
              props.name.trim().length === 0 ||
              props.selectedModels.length === 0
            }
          >
            {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
            创建 Key
          </button>
        </form>
      </div>
    </Modal>
  );
}
