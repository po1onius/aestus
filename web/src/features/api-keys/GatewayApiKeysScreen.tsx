import { useTenantLimits } from "../tenants/useTenantLimits";
import { TenantLimitNotice } from "../tenants/TenantLimitNotice";
import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useRequestScope } from "../../api/useRequestScope";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { ModelWhitelistDialog } from "../../components/ModelWhitelistDialog";
import { dashboardListPageSize, gatewayApiKeysPath, pluginSuitesPath, providerGroupsPath } from "../../config";
import { ApiKeyCreateDialog, type ApiKeyCreateDialogInput } from "../../features/api-keys/ApiKeyCreateDialog";
import { ApiKeyPluginDialog, type ApiKeyPluginDialogInput } from "../../features/api-keys/ApiKeyPluginDialog";
import { showErrorToast } from "../../lib/errors";
import { initialListPageState, listPagePath, ListPageState, pageStateFrom } from "../../lib/pagination";
import { utf8ByteLength } from "../../lib/validation";
import { GatewayApiKeysPage } from "../../pages/GatewayApiKeysPage";
import type { ApiKey, DeleteApiKeyResponse, ListApiKeysResponse, PluginSuiteSummary, ProviderGroup } from "../../types";
import { useDashboard, type PageProps } from "../dashboard/context";
import { useConfirmation } from "../dashboard/useConfirmation";

export function GatewayApiKeysScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser } = useDashboard();
  const tenantPolicy = useTenantLimits(authToken, refreshRevision);
  const { requestJson, isActiveAuthToken, beginRequest } = useRequestScope(authToken);
  const { confirmationRequest, setConfirmationRequest, confirmationSubmitting, closeConfirmationDialog, confirmRequestedAction } = useConfirmation();
  const [apiKeys, setApiKeys] = useState<ApiKey[]>([]);

  const [pluginOptions, setPluginOptions] = useState<PluginSuiteSummary[]>([]);

  const [providerGroupOptions, setProviderGroupOptions] = useState<ProviderGroup[]>([]);

  const [apiKeysPage, setApiKeysPage] = useState<ListPageState>(initialListPageState);

  const [apiKeysLoading, setApiKeysLoading] = useState(true);

  const [apiKeySaving, setApiKeySaving] = useState(false);

  const [apiKeyUpdatingId, setApiKeyUpdatingId] = useState<string | null>(null);

  const [apiKeyCreateOpen, setApiKeyCreateOpen] = useState(false);

  const [apiKeyModelsTarget, setApiKeyModelsTarget] = useState<ApiKey | null>(null);

  const [apiKeyPluginTarget, setApiKeyPluginTarget] = useState<ApiKey | null>(null);

  const [apiKeyPluginSaving, setApiKeyPluginSaving] = useState(false);

  async function loadProviderGroupOptions() {
    const token = authToken;
    const request = beginRequest("loadProviderGroupOptions");
    if (!token) {
      setProviderGroupOptions([]);
      return;
    }
    try {
      const groups = await requestJson<ProviderGroup[]>(`${providerGroupsPath}/options`, { signal: request.signal }, token);
      if (request.isCurrent()) {
        setProviderGroupOptions(groups);
      }
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("Provider 分组选项加载失败", error);
      }
    }
  }

  async function loadApiKeys(offset = apiKeysPage.offset) {
    const token = authToken;
    const request = beginRequest("loadApiKeys");
    if (!token) {
      setApiKeys([]);
      setApiKeysPage(initialListPageState());
      setApiKeysLoading(false);
      return;
    }

    setApiKeysLoading(true);
    try {
      const data = await requestJson<ListApiKeysResponse>(listPagePath(gatewayApiKeysPath, offset), { signal: request.signal }, token);
      if (!request.isCurrent()) {
        return;
      }
      setApiKeys(data.items);
      setApiKeysPage(pageStateFrom(data));
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("API Key 加载失败", error);
      }
    } finally {
      if (request.isCurrent()) {
        setApiKeysLoading(false);
      }
    }
  }

  async function loadPluginOptions() {
    const token = authToken;
    const request = beginRequest("loadPluginOptions");
    if (!token || currentUser?.role === "platform_admin") {
      setPluginOptions([]);
      return;
    }
    try {
      const data = await requestJson<PluginSuiteSummary[]>(
        `${pluginSuitesPath}/options`,
        { signal: request.signal },
        token,
      );
      if (request.isCurrent()) setPluginOptions(data);
    } catch (error) {
      if (request.isCurrent()) showErrorToast("插件选项加载失败", error);
    }
  }

  function openApiKeyCreateDialog() {
    setApiKeyCreateOpen(true);
  }

  function closeApiKeyCreateDialog() {
    if (apiKeySaving) {
      return;
    }
    setApiKeyCreateOpen(false);
  }

  function closeApiKeyModelsDialog() {
    if (apiKeyModelsTarget?.id === apiKeyUpdatingId) {
      return;
    }
    setApiKeyModelsTarget(null);
  }

  function openApiKeyPluginDialog(apiKey: ApiKey) {
    setApiKeyPluginTarget(apiKey);
  }

  function closeApiKeyPluginDialog() {
    if (apiKeyPluginSaving) {
      return;
    }
    setApiKeyPluginTarget(null);
  }

  async function submitApiKey({ name: apiKeyName, selectedModels: apiKeyAllowedModels, groupId: selectedProviderGroupId, pluginSuiteId: selectedPluginSuiteId }: ApiKeyCreateDialogInput) {
    const token = authToken;
    if (!token) {
      return;
    }
    if (!selectedProviderGroupId) {
      toast.error("API Key 创建失败", { description: "请选择 Provider 分组。" });
      return;
    }

    const name = apiKeyName.trim();
    const allowedModels = apiKeyAllowedModels;
    const selectedGroup = providerGroupOptions.find(
      (group) => group.id === selectedProviderGroupId,
    );
    const selectedPlugin = pluginOptions.find(
      (plugin) => plugin.id === selectedPluginSuiteId,
    );
    if (utf8ByteLength(name) > 128) {
      toast.error("API Key 创建失败", { description: "名称不能超过 128 字节。" });
      return;
    }
    if (allowedModels.length === 0) {
      toast.error("API Key 创建失败", { description: "请至少选择一个白名单模型。" });
      return;
    }
    if (
      !selectedGroup ||
      allowedModels.some((model) => !selectedGroup.allowed_models.includes(model))
    ) {
      toast.error("API Key 创建失败", {
        description: "模型白名单只能从当前 Provider 分组的限制模型中选择。",
      });
      return;
    }
    if (allowedModels.length > 128 || allowedModels.some((model) => utf8ByteLength(model) > 256)) {
      toast.error("API Key 创建失败", {
        description: "模型白名单最多 128 项，每个模型名最多 256 字节。",
      });
      return;
    }
    if (
      selectedPluginSuiteId &&
      (!selectedPlugin ||
        !selectedPlugin.enabled ||
        selectedPlugin.provider !== selectedGroup.provider)
    ) {
      toast.error("API Key 创建失败", {
        description: "插件套件必须属于当前 Provider 且处于启用状态。",
      });
      return;
    }

    setApiKeySaving(true);
    try {
      await requestJson<ApiKey>(gatewayApiKeysPath, {
        method: "POST",
        body: JSON.stringify({
          name,
          group_id: selectedProviderGroupId,
          allowed_models: allowedModels,
          plugin_suite_id: selectedPluginSuiteId || null,
        }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeyCreateOpen(false);
      await loadApiKeys(0);

      if (isActiveAuthToken(authToken)) toast.success("API Key 已创建");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("API Key 创建失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setApiKeySaving(false);
      }
    }
  }

  async function submitApiKeyPlugin({ pluginSuiteId: apiKeyPluginSuiteId }: ApiKeyPluginDialogInput) {
    const token = authToken;
    const target = apiKeyPluginTarget;
    if (!token || !target) {
      return;
    }
    if (!target.group) {
      toast.error("分组已删除，Key 无效，不能编辑");
      return;
    }

    const selectedPlugin = pluginOptions.find(
      (plugin) => plugin.id === apiKeyPluginSuiteId,
    );
    if (
      apiKeyPluginSuiteId &&
      (!selectedPlugin ||
        !selectedPlugin.enabled ||
        selectedPlugin.provider !== target.group.provider)
    ) {
      toast.error("插件绑定更新失败", {
        description: "请选择与 API Key Provider 一致的启用套件。",
      });
      return;
    }

    setApiKeyPluginSaving(true);
    try {
      const updated = await requestJson<ApiKey>(`${gatewayApiKeysPath}/${target.id}/plugin-suite`, {
        method: "PUT",
        body: JSON.stringify({ plugin_suite_id: apiKeyPluginSuiteId || null }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      setApiKeyPluginTarget(null);
      if (isActiveAuthToken(authToken)) toast.success(updated.plugin ? "插件绑定已更新" : "插件绑定已解除");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("插件绑定更新失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setApiKeyPluginSaving(false);
      }
    }
  }

  async function updateApiKeyModels(apiKey: ApiKey, allowedModels: string[]): Promise<boolean> {
    const token = authToken;
    if (!token) return false;
    if (allowedModels.length === 0 || allowedModels.length > 128) {
      toast.error("API Key 模型更新失败", { description: "模型白名单必须包含 1 到 128 项。" });
      return false;
    }
    if (
      allowedModels.some(
        (model) =>
          utf8ByteLength(model.trim()) > 256 ||
          !apiKey.group_allowed_models.includes(model),
      )
    ) {
      toast.error("API Key 模型更新失败", {
        description: "模型必须来自当前 Provider 分组白名单，且每项最多 256 字节。",
      });
      return false;
    }

    setApiKeyUpdatingId(apiKey.id);
    try {
      const updated = await requestJson<ApiKey>(`${gatewayApiKeysPath}/${apiKey.id}/models`, {
        method: "PUT",
        body: JSON.stringify({ allowed_models: allowedModels }),
      }, token);
      if (!isActiveAuthToken(token)) return false;
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      if (isActiveAuthToken(authToken)) toast.success("API Key 模型白名单已更新");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("API Key 模型更新失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setApiKeyUpdatingId(null);
    }
  }

  async function toggleApiKeyEnabled(apiKey: ApiKey) {
    const token = authToken;
    if (!token) {
      return;
    }

    setApiKeyUpdatingId(apiKey.id);
    try {
      const updated = await requestJson<ApiKey>(`${gatewayApiKeysPath}/${apiKey.id}/enabled`, {
        method: "POST",
        body: JSON.stringify({ enabled: !apiKey.enabled }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));

      if (isActiveAuthToken(authToken)) toast.success(apiKey.enabled ? "API Key 已禁用" : "API Key 已启用");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast(apiKey.enabled ? "API Key 禁用失败" : "API Key 启用失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setApiKeyUpdatingId(null);
      }
    }
  }

  function requestDeleteApiKey(apiKey: ApiKey) {
    setConfirmationRequest({
      title: "删除网关 API Key",
      description: `将永久删除网关 API Key“${apiKey.name}”，后续请求将立即无法再使用该凭证。Provider 分组、上游资源和历史请求日志不受影响。`,
      confirmLabel: "删除 API Key",
      pendingLabel: "正在删除",
      onConfirm: () => deleteApiKey(apiKey),
    });
  }

  async function deleteApiKey(apiKey: ApiKey) {
    const token = authToken;
    if (!token) {
      return;
    }

    setApiKeyUpdatingId(apiKey.id);
    try {
      const deleted = await requestJson<DeleteApiKeyResponse>(`${gatewayApiKeysPath}/${apiKey.id}`, {
        method: "DELETE",
      }, token);
      if (!isActiveAuthToken(token)) return;
      setApiKeys((items) => items.filter((item) => item.id !== deleted.id));
      const nextOffset =
        apiKeys.length === 1 && apiKeysPage.offset > 0
          ? Math.max(0, apiKeysPage.offset - dashboardListPageSize)
          : apiKeysPage.offset;
      await Promise.all([
        loadApiKeys(nextOffset),

      ]);
      if (isActiveAuthToken(authToken)) toast.success("API Key 已删除");
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("API Key 删除失败", error);
    } finally {
      if (isActiveAuthToken(token)) setApiKeyUpdatingId(null);
    }
  }

  async function copyApiKey(apiKey: ApiKey) {
    try {
      await navigator.clipboard.writeText(apiKey.api_key);
      if (isActiveAuthToken(authToken)) toast.success("API Key 已复制");
    } catch (error) {
      showErrorToast("API Key 复制失败", error);
    }
  }
  useEffect(() => { void loadApiKeys(); void loadProviderGroupOptions(); void loadPluginOptions(); }, [refreshRevision]);
  useEffect(() => { onLoadingChange(apiKeysLoading); return () => onLoadingChange(false); }, [apiKeysLoading, onLoadingChange]);
  return <><TenantLimitNotice {...tenantPolicy} resource="keys" /><GatewayApiKeysPage
    apiKeys={apiKeys}
    loading={apiKeysLoading}
    updatingId={apiKeyUpdatingId}
    offset={apiKeysPage.offset}
    pageSize={dashboardListPageSize}
    nextOffset={apiKeysPage.nextOffset}
    onCreate={openApiKeyCreateDialog}
    onEditModels={setApiKeyModelsTarget}
    onEditPlugin={openApiKeyPluginDialog}
    onToggleEnabled={toggleApiKeyEnabled}
    onDelete={requestDeleteApiKey}
    onCopy={copyApiKey}
    onPageChange={loadApiKeys}
  /><AnimatePresence>{apiKeyCreateOpen && (
    <ApiKeyCreateDialog
      key="api-key-create"
      saving={apiKeySaving}
      groups={providerGroupOptions}
      plugins={pluginOptions}
      onSubmit={submitApiKey}
      onClose={closeApiKeyCreateDialog}
    />
  )}
      {apiKeyModelsTarget && (
        <ModelWhitelistDialog
          key={`api-key-models-${apiKeyModelsTarget.id}`}
          titleId="apiKeyModelsTitle"
          title="修改 API Key 模型"
          description={`修改 API Key“${apiKeyModelsTarget.name}”的模型白名单。`}
          models={apiKeyModelsTarget.allowed_models}
          availableModels={apiKeyModelsTarget.group_allowed_models}
          saving={apiKeyUpdatingId === apiKeyModelsTarget.id}
          onSave={(models) => updateApiKeyModels(apiKeyModelsTarget, models)}
          onClose={closeApiKeyModelsDialog}
        />
      )}
      {apiKeyPluginTarget && (
        <ApiKeyPluginDialog
          key={`api-key-plugin-${apiKeyPluginTarget.id}`}
          apiKey={apiKeyPluginTarget}
          plugins={pluginOptions}
          saving={apiKeyPluginSaving}
          onSubmit={submitApiKeyPlugin}
          onClose={closeApiKeyPluginDialog}
        />
      )}{confirmationRequest && <ConfirmDialog
        key="confirmation"
        title={confirmationRequest.title}
        description={confirmationRequest.description}
        confirmLabel={confirmationRequest.confirmLabel}
        pendingLabel={confirmationRequest.pendingLabel}
        pending={confirmationSubmitting}
        onConfirm={confirmRequestedAction}
        onClose={closeConfirmationDialog}
      />}</AnimatePresence></>;
}
