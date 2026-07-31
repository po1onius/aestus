import { KeyRound, Loader2, Plus, UserRound, X } from "lucide-react";
import { useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  buttonSecondary,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
} from "../../lib/ui";
import type { UnassignedProviderResource } from "../../types";

const MAX_GROUP_MODELS = 128;

interface ProviderGroupCreateDialogProps {
  providerLabel: string;
  saving: boolean;
  resourcesLoading: boolean;
  resources: UnassignedProviderResource[];
  onCreate: (
    name: string,
    models: string[],
    accountIds: string[],
    apiKeyIds: string[],
  ) => Promise<boolean>;
  onClose: () => void;
}

/** 新增分组弹窗只负责采集创建所需字段，分组列表与行级管理保留在 Provider 主页面。 */
export function ProviderGroupCreateDialog({
  providerLabel,
  saving,
  resourcesLoading,
  resources,
  onCreate,
  onClose,
}: ProviderGroupCreateDialogProps) {
  const [name, setName] = useState("");
  const [modelInput, setModelInput] = useState("");
  const [confirmedModels, setConfirmedModels] = useState<string[]>([]);
  const [selectedResourceIds, setSelectedResourceIds] = useState<string[]>([]);
  const normalizedModelInput = modelInput.trim();
  const modelAlreadyConfirmed = confirmedModels.includes(normalizedModelInput);
  const modelLimitReached = confirmedModels.length >= MAX_GROUP_MODELS;
  const canAddModel =
    !saving && Boolean(normalizedModelInput) && !modelAlreadyConfirmed && !modelLimitReached;
  const selectedResourceIdSet = new Set(selectedResourceIds);
  const accountResources = resources.filter((resource) => resource.resource_type === "account");
  const apiKeyResources = resources.filter((resource) => resource.resource_type === "api_key");

  /** 只有经过“添加”确认的模型才会进入最终提交数据，避免未完成的输入被意外提交。 */
  function addModel() {
    if (!canAddModel) {
      return;
    }
    setConfirmedModels((currentModels) => [...currentModels, normalizedModelInput]);
    setModelInput("");
  }

  function handleModelInputKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") {
      return;
    }
    event.preventDefault();
    addModel();
  }

  function removeModel(model: string) {
    if (saving) {
      return;
    }
    setConfirmedModels((currentModels) =>
      currentModels.filter((currentModel) => currentModel !== model),
    );
  }

  function toggleResource(id: string) {
    if (saving) {
      return;
    }
    setSelectedResourceIds((currentIds) =>
      currentIds.includes(id)
        ? currentIds.filter((currentId) => currentId !== id)
        : [...currentIds, id],
    );
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const normalizedName = name.trim();
    if (
      !normalizedName ||
      confirmedModels.length === 0 ||
      selectedResourceIds.length === 0 ||
      saving
    ) {
      return;
    }
    const accountIds = accountResources
      .filter((resource) => selectedResourceIdSet.has(resource.id))
      .map((resource) => resource.id);
    const apiKeyIds = apiKeyResources
      .filter((resource) => selectedResourceIdSet.has(resource.id))
      .map((resource) => resource.id);
    if (await onCreate(normalizedName, confirmedModels, accountIds, apiKeyIds)) {
      onClose();
    }
  }

  return (
    <Modal
      titleId="providerGroupCreateTitle"
      title={`添加 ${providerLabel} 分组`}
      description="配置限制模型，并选择至少一个当前尚未分组的账号或官方 API Key。"
      closeDisabled={saving}
      onClose={onClose}
    >
      <form className="grid gap-4" onSubmit={submit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>
            分组名称<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="例如 production"
            maxLength={128}
            autoFocus
          />
        </label>
        <div className={fieldStack}>
          <span className={fieldLabel}>
            已确认模型
            <span className={requiredMark}>*</span>
            <span className="ml-2 font-normal text-slate-500 dark:text-slate-400">
              {confirmedModels.length}/{MAX_GROUP_MODELS}
            </span>
          </span>
          <div className="flex min-h-20 flex-wrap content-start gap-2 rounded-lg border border-slate-200 bg-slate-50/70 p-3 dark:border-slate-800 dark:bg-slate-950/70">
            {confirmedModels.length === 0 ? (
              <span className="text-sm text-slate-400 dark:text-slate-600">尚未添加模型</span>
            ) : (
              confirmedModels.map((model) => (
                <span
                  key={model}
                  className="inline-flex h-fit max-w-full items-center gap-1.5 rounded-full bg-indigo-50 px-2.5 py-1 text-sm font-medium text-indigo-800 ring-1 ring-inset ring-indigo-200 dark:bg-indigo-950/50 dark:text-indigo-200 dark:ring-indigo-800"
                >
                  <span className="truncate">{model}</span>
                  <button
                    type="button"
                    className="shrink-0 rounded-full p-0.5 transition-colors hover:bg-indigo-200/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/40 disabled:pointer-events-none disabled:opacity-50 dark:hover:bg-indigo-800/70"
                    aria-label={`删除已确认模型 ${model}`}
                    disabled={saving}
                    onClick={() => removeModel(model)}
                  >
                    <X size={14} aria-hidden="true" />
                  </button>
                </span>
              ))
            )}
          </div>
          <div className="flex items-stretch gap-2">
            <input
              className={inputClass}
              value={modelInput}
              onChange={(event) => setModelInput(event.target.value)}
              onKeyDown={handleModelInputKeyDown}
              placeholder="输入模型名，例如 gpt-5.4"
              aria-label="待添加的模型名称"
              disabled={saving || modelLimitReached}
              maxLength={256}
            />
            <button
              type="button"
              className={`${buttonSecondary} shrink-0`}
              disabled={!canAddModel}
              onClick={addModel}
            >
              <Plus size={16} aria-hidden="true" />
              添加
            </button>
          </div>
          <span className={fieldHelp}>
            {modelLimitReached
              ? `最多添加 ${MAX_GROUP_MODELS} 个模型。`
              : modelAlreadyConfirmed
                ? "该模型已经添加。"
                : "输入一个模型名后点击“添加”，也可以按 Enter 确认。"}
          </span>
        </div>
        <div className={fieldStack}>
          <span className={fieldLabel}>
            初始资源<span className={requiredMark}>*</span>
            <span className="ml-2 font-normal text-slate-500 dark:text-slate-400">
              已选 {selectedResourceIds.length}
            </span>
          </span>
          {resourcesLoading ? (
            <div className="flex min-h-24 items-center justify-center gap-2 rounded-lg border border-slate-200 text-sm text-slate-500 dark:border-slate-800 dark:text-slate-400">
              <Loader2 className={spinnerClass} size={18} />
              正在加载未分组资源
            </div>
          ) : resources.length === 0 ? (
            <div className="rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-300">
              当前没有未分组资源，请先导入账号或官方 API Key。
            </div>
          ) : (
            <div className="grid max-h-64 gap-3 overflow-y-auto rounded-lg border border-slate-200 bg-slate-50/70 p-3 dark:border-slate-800 dark:bg-slate-950/70">
              <ResourceSection
                title="OAuth 账号"
                resources={accountResources}
                selectedIds={selectedResourceIdSet}
                saving={saving}
                icon="account"
                onToggle={toggleResource}
              />
              <ResourceSection
                title="官方 API Key"
                resources={apiKeyResources}
                selectedIds={selectedResourceIdSet}
                saving={saving}
                icon="api_key"
                onToggle={toggleResource}
              />
            </div>
          )}
          <span className={fieldHelp}>
            资源一次只能属于一个分组；已分组资源可在资源列表中迁移或设为未分组。
          </span>
        </div>
        <button
          className={`${buttonPrimary} w-full`}
          disabled={
            saving ||
            resourcesLoading ||
            !name.trim() ||
            confirmedModels.length === 0 ||
            selectedResourceIds.length === 0
          }
        >
          {saving ? <Loader2 className={spinnerClass} size={18} /> : <Plus size={18} />}
          添加分组
        </button>
      </form>
    </Modal>
  );
}

interface ResourceSectionProps {
  title: string;
  resources: UnassignedProviderResource[];
  selectedIds: Set<string>;
  saving: boolean;
  icon: UnassignedProviderResource["resource_type"];
  onToggle: (id: string) => void;
}

function ResourceSection({
  title,
  resources,
  selectedIds,
  saving,
  icon,
  onToggle,
}: ResourceSectionProps) {
  if (resources.length === 0) {
    return null;
  }
  const Icon = icon === "account" ? UserRound : KeyRound;
  return (
    <section className="grid gap-2" aria-label={title}>
      <strong className="text-xs font-semibold text-slate-600 dark:text-slate-300">{title}</strong>
      {resources.map((resource) => (
        <label
          key={resource.id}
          className="grid cursor-pointer grid-cols-[auto_auto_minmax(0,1fr)] items-start gap-2 rounded-lg border border-slate-200 bg-white p-2.5 text-sm transition hover:border-indigo-300 dark:border-slate-800 dark:bg-slate-900 dark:hover:border-indigo-700"
        >
          <input
            className="mt-1 size-4 accent-indigo-600"
            type="checkbox"
            checked={selectedIds.has(resource.id)}
            disabled={saving}
            onChange={() => onToggle(resource.id)}
          />
          <Icon className="mt-0.5 text-slate-500 dark:text-slate-400" size={17} />
          <span className="min-w-0">
            <span className="block font-medium text-slate-800 dark:text-slate-100">
              {resource.display_name}
            </span>
            <span className="block truncate text-xs text-slate-500 dark:text-slate-400" title={resource.detail}>
              {resource.detail}
            </span>
            <code className="block truncate text-[10px] text-slate-400" title={resource.id}>
              {resource.id}
            </code>
          </span>
        </label>
      ))}
    </section>
  );
}
