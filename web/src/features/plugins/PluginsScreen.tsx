import { useTenantLimits } from "../tenants/useTenantLimits";
import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useRequestScope } from "../../api/useRequestScope";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { pluginsPath, pluginSuitesPath } from "../../config";
import { PluginCreateDialog, type CreatePluginInput } from "../../features/plugins/PluginCreateDialog";
import { PluginSuiteCreateDialog, type CreatePluginSuiteInput } from "../../features/plugins/PluginSuiteCreateDialog";
import { canManagePlugin } from "../../features/plugins/access";
import { suiteSlotFields } from "../../features/plugins/slots";
import { showErrorToast } from "../../lib/errors";
import { PluginsPage } from "../../pages/PluginsPage";
import type { DeletePluginResponse, PluginDeletionImpact, PluginSuiteSummary, PluginSummary } from "../../types";
import { useDashboard, type PageProps } from "../dashboard/context";
import { useConfirmation } from "../dashboard/useConfirmation";
import { validPluginText } from "../plugins/validation";

export function PluginsScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser } = useDashboard();
  const tenantPolicy = useTenantLimits(authToken, refreshRevision, currentUser.role === "platform_admin");
  const canUploadWasm = currentUser.role === "platform_admin"
    || (currentUser.role === "tenant_owner" && tenantPolicy.limits?.owner_can_upload_wasm === true);
  const { requestJson, requestFormData, isActiveAuthToken, beginRequest } = useRequestScope(authToken);
  const { confirmationRequest, setConfirmationRequest, confirmationSubmitting, closeConfirmationDialog, confirmRequestedAction } = useConfirmation();
  const [plugins, setPlugins] = useState<PluginSummary[]>([]);

  const [pluginSuites, setPluginSuites] = useState<PluginSuiteSummary[]>([]);

  const [pluginsLoading, setPluginsLoading] = useState(false);

  const [pluginSavingId, setPluginSavingId] = useState<string | null>(null);

  const [pluginCreateOpen, setPluginCreateOpen] = useState(false);

  const [pluginSuiteCreateOpen, setPluginSuiteCreateOpen] = useState(false);

  async function loadPlugins() {
    const token = authToken;
    const request = beginRequest("plugins");
    if (!token || (currentUser?.role !== "tenant_owner" && currentUser?.role !== "platform_admin")) {
      setPlugins([]);
      setPluginSuites([]);
      setPluginsLoading(false);
      return;
    }

    setPluginsLoading(true);
    try {
      const [data, suites] = await Promise.all([
        requestJson<PluginSummary[]>(pluginsPath, { signal: request.signal }, token),
        requestJson<PluginSuiteSummary[]>(pluginSuitesPath, { signal: request.signal }, token),
      ]);
      if (request.isCurrent()) {
        setPlugins(data);
        setPluginSuites(suites);
      }
    } catch (error) {
      if (request.isCurrent()) showErrorToast("插件加载失败", error);
    } finally {
      if (request.isCurrent()) setPluginsLoading(false);
    }
  }

  function closePluginCreateDialog() {
    if (pluginSavingId === "create") {
      return;
    }
    setPluginCreateOpen(false);
  }

  function closePluginSuiteCreateDialog() {
    if (pluginSavingId !== null) {
      return;
    }
    setPluginSuiteCreateOpen(false);
  }

  async function createPlugin(input: CreatePluginInput): Promise<boolean> {
    const token = authToken;
    if (!token || !canUploadWasm) return false;
    if (!validPluginText(input.name, input.description)) return false;
    if (input.file.size === 0 || input.file.size > 8 * 1024 * 1024) {
      toast.error("插件上传失败", { description: "WASM 文件大小必须在 1 B 到 8 MiB 之间。" });
      return false;
    }
    const formData = new FormData();
    formData.set("name", input.name.trim());
    formData.set("description", input.description.trim());
    formData.set("provider", input.provider);
    formData.set("slot", input.slot);
    formData.set("wasm_file", input.file);
    setPluginSavingId("create");
    try {
      await requestFormData<PluginSummary>(pluginsPath, formData, { method: "POST" }, token);
      if (!isActiveAuthToken(token)) return false;
      await loadPlugins();
      if (isActiveAuthToken(authToken)) toast.success("WASM 插件已上传，可以创建套件");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件上传失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function createPluginSuite(input: CreatePluginSuiteInput): Promise<boolean> {
    const token = authToken;
    if (!token || !validPluginText(input.name, input.description)) return false;
    if (!suiteSlotFields.some(({ field }) => input[field])) {
      toast.error("套件至少需要选择一个插件");
      return false;
    }
    setPluginSavingId("create-suite");
    try {
      await requestJson<PluginSuiteSummary>(pluginSuitesPath, {
        method: "POST",
        body: JSON.stringify({ ...input, name: input.name.trim(), description: input.description.trim() }),
      }, token);
      if (!isActiveAuthToken(token)) return false;
      await loadPlugins();
      if (isActiveAuthToken(authToken)) toast.success("套件已创建");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("套件创建失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function togglePluginEnabled(suite: PluginSuiteSummary) {
    const token = authToken;
    if (!token || !canManagePlugin(currentUser, suite.tenant_id)) return;
    setPluginSavingId(suite.id);
    try {
      const updated = await requestJson<PluginSuiteSummary[]>(`${pluginSuitesPath}/${suite.id}/enabled`, {
        method: "PUT", body: JSON.stringify({ enabled: !suite.enabled }),
      }, token);
      if (!isActiveAuthToken(token)) return;
      setPluginSuites(updated);

      if (isActiveAuthToken(authToken)) toast.success(suite.enabled ? "套件已停用" : "套件已启用");
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("套件状态更新失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  function requestDeletePlugin(plugin: PluginSummary) {
    void previewPluginDeletion(plugin, false);
  }

  function requestDeletePluginSuite(suite: PluginSuiteSummary) {
    void previewPluginDeletion(suite, true);
  }

  async function previewPluginDeletion(resource: PluginSummary | PluginSuiteSummary, isSuite: boolean) {
    const token = authToken;
    if (!token || !canManagePlugin(currentUser, resource.tenant_id)) return;
    setPluginSavingId(resource.id);
    try {
      const base = isSuite ? `${pluginSuitesPath}/${resource.id}` : `${pluginsPath}/${resource.id}`;
      const impact = await requestJson<PluginDeletionImpact>(`${base}/deletion-impact`, undefined, token);
      if (!isActiveAuthToken(token)) return;
      setConfirmationRequest({
        title: isSuite ? "删除套件" : "删除 WASM 插件",
        description: `将永久删除${resource.tenant_id === null ? "平台公共" : "本租户"}${isSuite ? "套件" : "插件"}“${resource.name}”。${isSuite ? "独立插件会保留。" : "所有引用它的套件也会删除，包括各租户的私有套件。"}当前影响 ${impact.suite_count} 个套件、${impact.affected_tenant_count} 个租户、${impact.affected_gateway_api_key_count} 个网关 Key。Key 会保留失效引用，后续 Responses / Messages 请求将被拒绝，需要重新绑定或解除绑定。删除时按最新依赖执行。`,
        confirmLabel: isSuite ? "删除套件" : "删除插件及关联套件",
        pendingLabel: "正在删除",
        onConfirm: () => deletePluginResource(resource.id, isSuite),
      });
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("删除影响加载失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function deletePluginResource(id: string, isSuite: boolean) {
    const token = authToken;
    if (!token || (currentUser?.role !== "tenant_owner" && currentUser?.role !== "platform_admin")) return;
    setPluginSavingId(id);
    try {
      const deleted = await requestJson<DeletePluginResponse>(
        isSuite ? `${pluginSuitesPath}/${id}` : `${pluginsPath}/${id}`,
        { method: "DELETE" }, token,
      );
      if (!isActiveAuthToken(token)) return;
      await loadPlugins();
      if (isActiveAuthToken(authToken)) toast.success(isSuite ? "套件已删除" : "插件已删除", {
        description: `已删除 ${deleted.deleted_suite_count} 个套件，影响 ${deleted.affected_tenant_count} 个租户，${deleted.affected_gateway_api_key_count} 个网关 Key 的套件绑定失效。`,
      });
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("删除失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }
  useEffect(() => { void loadPlugins(); }, [refreshRevision]);
  useEffect(() => { onLoadingChange(pluginsLoading); return () => onLoadingChange(false); }, [pluginsLoading, onLoadingChange]);
  return <><PluginsPage
    user={currentUser}
    canUploadWasm={canUploadWasm}
    uploadRestriction={tenantPolicy.loading ? "正在读取上传权限…" : !tenantPolicy.limits
      ? "上传权限加载失败，请刷新页面。" : "平台未允许本租户 owner 上传 WASM；已有插件和套件仍可使用、管理。"}
    plugins={plugins}
    suites={pluginSuites}
    loading={pluginsLoading}
    savingId={pluginSavingId}
    onAddPlugin={() => { if (canUploadWasm) setPluginCreateOpen(true); }}
    onAddSuite={() => setPluginSuiteCreateOpen(true)}
    onToggleEnabled={togglePluginEnabled}
    onDeletePlugin={requestDeletePlugin}
    onDeleteSuite={requestDeletePluginSuite}
  /><AnimatePresence>{pluginCreateOpen && (
    <PluginCreateDialog
      isPlatformAdmin={currentUser.role === "platform_admin"}
      key="plugin-create"
      saving={pluginSavingId === "create"}
      onCreate={createPlugin}
      onClose={closePluginCreateDialog}
    />
  )}
      {pluginSuiteCreateOpen && (
        <PluginSuiteCreateDialog
          isPlatformAdmin={currentUser.role === "platform_admin"}
          plugins={plugins}
          saving={pluginSavingId === "create-suite"}
          onCreate={createPluginSuite}
          onClose={closePluginSuiteCreateDialog}
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
