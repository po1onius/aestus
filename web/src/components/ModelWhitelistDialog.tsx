import { Loader2, Plus, Save, X } from "lucide-react";
import { useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { Modal } from "./Modal";
import {
  buttonPrimary,
  buttonSecondary,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  spinnerClass,
} from "../lib/ui";

const MAX_MODELS = 128;

interface ModelWhitelistDialogProps {
  titleId: string;
  title: string;
  description: string;
  models: string[];
  /**
   * 传入候选集合时使用复选框，适合 Key 从所属分组范围内收窄权限；不传时允许管理员
   * 输入任意模型名，适合维护 Provider 分组的上层授权边界。
   */
  availableModels?: string[];
  saving: boolean;
  onSave: (models: string[]) => Promise<boolean>;
  onClose: () => void;
}

/** 分组和网关 Key 共用的模型白名单编辑器，只提交完整集合，不执行任何级联更新。 */
export function ModelWhitelistDialog(props: ModelWhitelistDialogProps) {
  const [models, setModels] = useState(() => [...props.models]);
  const [modelInput, setModelInput] = useState("");
  const normalizedInput = modelInput.trim();
  const selectedModels = new Set(models);
  const availableModels = props.availableModels;
  const availableModelSet = new Set(availableModels ?? []);
  const unavailableModels = availableModels
    ? models.filter((model) => !availableModelSet.has(model))
    : [];
  const canAdd =
    !props.saving &&
    Boolean(normalizedInput) &&
    !selectedModels.has(normalizedInput) &&
    models.length < MAX_MODELS;
  const unchanged = sameModels(models, props.models);

  function addModel() {
    if (!canAdd) return;
    setModels((current) => [...current, normalizedInput]);
    setModelInput("");
  }

  function handleInputKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    addModel();
  }

  function toggleModel(model: string) {
    if (props.saving) return;
    setModels((current) =>
      current.includes(model) ? current.filter((item) => item !== model) : [...current, model],
    );
  }

  function removeModel(model: string) {
    if (props.saving) return;
    setModels((current) => current.filter((item) => item !== model));
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (props.saving || models.length === 0 || unchanged || unavailableModels.length > 0) return;
    if (await props.onSave(models)) props.onClose();
  }

  return (
    <Modal
      titleId={props.titleId}
      title={props.title}
      description={props.description}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={submit}>
        <div className={fieldStack}>
          <span className={fieldLabel}>
            模型白名单
            <span className="ml-2 font-normal text-slate-500 dark:text-slate-400">
              {models.length}/{MAX_MODELS}
            </span>
          </span>

          {availableModels ? (
            <div className="grid max-h-72 gap-2 overflow-y-auto rounded-xl border border-slate-200 p-3 dark:border-slate-700">
              {availableModels.map((model) => (
                <label
                  key={model}
                  className="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-2 text-sm text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800/70"
                >
                  <input
                    type="checkbox"
                    className="accent-indigo-600"
                    checked={selectedModels.has(model)}
                    disabled={props.saving}
                    onChange={() => toggleModel(model)}
                  />
                  <code className="break-all font-mono text-xs">{model}</code>
                </label>
              ))}
            </div>
          ) : (
            <>
              <div className="flex min-h-20 flex-wrap content-start gap-2 rounded-lg border border-slate-200 bg-slate-50/70 p-3 dark:border-slate-800 dark:bg-slate-950/70">
                {models.map((model) => (
                  <span
                    key={model}
                    className="inline-flex h-fit max-w-full items-center gap-1.5 rounded-full bg-indigo-50 px-2.5 py-1 text-sm font-medium text-indigo-800 ring-1 ring-inset ring-indigo-200 dark:bg-indigo-950/50 dark:text-indigo-200 dark:ring-indigo-800"
                  >
                    <span className="truncate">{model}</span>
                    <button
                      type="button"
                      className="shrink-0 rounded-full p-0.5 transition-colors hover:bg-indigo-200/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/40 disabled:pointer-events-none disabled:opacity-50 dark:hover:bg-indigo-800/70"
                      aria-label={`删除模型 ${model}`}
                      disabled={props.saving}
                      onClick={() => removeModel(model)}
                    >
                      <X size={14} aria-hidden="true" />
                    </button>
                  </span>
                ))}
              </div>
              <div className="flex items-stretch gap-2">
                <input
                  className={inputClass}
                  value={modelInput}
                  onChange={(event) => setModelInput(event.target.value)}
                  onKeyDown={handleInputKeyDown}
                  placeholder="输入模型名"
                  aria-label="待添加的模型名称"
                  disabled={props.saving || models.length >= MAX_MODELS}
                  maxLength={256}
                />
                <button
                  type="button"
                  className={`${buttonSecondary} shrink-0`}
                  disabled={!canAdd}
                  onClick={addModel}
                >
                  <Plus size={16} aria-hidden="true" />
                  添加
                </button>
              </div>
            </>
          )}

          {unavailableModels.length > 0 ? (
            <div className="rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-300">
              <p>以下模型已不在当前分组白名单中，保存前需要移除：</p>
              <div className="mt-2 flex flex-wrap gap-2">
                {unavailableModels.map((model) => (
                  <button
                    key={model}
                    type="button"
                    className="inline-flex items-center gap-1 rounded-full bg-amber-100 px-2.5 py-1 font-mono text-xs dark:bg-amber-900/60"
                    disabled={props.saving}
                    onClick={() => removeModel(model)}
                  >
                    {model}
                    <X size={13} />
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <span className={fieldHelp}>
              {availableModels
                ? "Key 白名单只能从当前分组模型中选择；网关请求仍会同时校验两层白名单。"
                : "修改分组白名单不会改写其下 Key；网关请求会实时同时校验两层白名单。"}
            </span>
          )}
        </div>

        <button
          className={`${buttonPrimary} w-full`}
          disabled={
            props.saving || models.length === 0 || unchanged || unavailableModels.length > 0
          }
        >
          {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
          保存模型白名单
        </button>
      </form>
    </Modal>
  );
}

function sameModels(left: string[], right: string[]) {
  if (left.length !== right.length) return false;
  const normalizedLeft = [...left].sort();
  const normalizedRight = [...right].sort();
  return normalizedLeft.every((model, index) => model === normalizedRight[index]);
}
