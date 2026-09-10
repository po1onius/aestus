import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { isProviderStateSyncError } from "../../api/client";
import { useRequestScope } from "../../api/useRequestScope";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { ModelWhitelistDialog } from "../../components/ModelWhitelistDialog";
import { accountProviderTabs, claudeAccountsPath, dashboardListPageSize, gptAccountsPath, providerGroupsPath } from "../../config";
import { AccountImportDialog } from "../../features/accounts/AccountImportDialog";
import { AccountQuotaDialog } from "../../features/accounts/AccountQuotaDialog";
import { ProviderGroupCreateDialog } from "../../features/accounts/ProviderGroupCreateDialog";
import { ProviderUpstreamApiKeyDialog, type ProviderUpstreamApiKeyDialogInput } from "../../features/accounts/ProviderUpstreamApiKeyDialog";
import { RateLimitResetDialog } from "../../features/accounts/RateLimitResetDialog";
import { RequestOverrideDialog } from "../../features/accounts/RequestOverrideDialog";
import { overrideEntriesToObject } from "../../features/accounts/utils";
import { errorMessageFrom, showErrorToast } from "../../lib/errors";
import { initialListPageState, listPagePath, ListPageState, pageStateFrom } from "../../lib/pagination";
import { isAscii, utf8ByteLength } from "../../lib/validation";
import { ProvidersPage } from "../../pages/ProvidersPage";
import type { AccountImportMode, AccountProviderKey, ClaudeAccount, ConsumeRateLimitResetCreditResponse, DeleteClaudeAccountResponse, DeleteGptAccountResponse, DeleteProviderGroupResponse, DeleteProviderUpstreamApiKeyResponse, GptAccount, GptAccountQuotaResult, ListAccountsResponse, ListClaudeAccountsResponse, ListProviderUpstreamApiKeysResponse, OauthAuthorizationResponse, OverrideEntry, ProviderCredentialTab, ProviderGroup, ProviderGroupSummary, ProviderUpstreamApiKey, RateLimitResetCredit, RateLimitResetCreditsResponse, RequestOverride, RequestOverrideTarget, UnassignedProviderResource, UpstreamApiKeyProvider } from "../../types";
import { AccountPageOffsets, asUpstreamApiKeyProvider, defaultUpstreamApiKeyBaseUrl, providerLabel, upstreamApiKeyPath } from "../accounts/resources";
import { useDashboard, type PageProps } from "../dashboard/context";
import { useConfirmation } from "../dashboard/useConfirmation";
import { useProviderGroups } from "./useProviderGroups";
import { useTenantResourceUsage } from "../tenants/useTenantResourceUsage";

export function ProvidersScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser, providerAccess } = useDashboard();
  const { usage: resourceUsage, loading: resourceUsageLoading, reload: reloadResourceUsage } = useTenantResourceUsage(authToken, refreshRevision, providerAccess.isOwner);
  const resourceLimitReached = resourceUsage !== null && resourceUsage.max_resources !== null && resourceUsage.resource_count >= resourceUsage.max_resources;
  const groupLimitReached = resourceUsage !== null && resourceUsage.max_provider_groups !== null && resourceUsage.provider_group_count >= resourceUsage.max_provider_groups;
  const { providerGroups, providerGroupsLoading, loadProviderGroups } = useProviderGroups(authToken);
  const { requestJson, isActiveAuthToken, beginRequest } = useRequestScope(authToken);
  const { confirmationRequest, setConfirmationRequest, confirmationSubmitting, closeConfirmationDialog, confirmRequestedAction } = useConfirmation();
  const [accounts, setAccounts] = useState<GptAccount[]>([]);

  const [claudeAccounts, setClaudeAccounts] = useState<ClaudeAccount[]>([]);

  const [gptUpstreamApiKeys, setGptUpstreamApiKeys] = useState<ProviderUpstreamApiKey[]>([]);

  const [claudeUpstreamApiKeys, setClaudeUpstreamApiKeys] =
    useState<ProviderUpstreamApiKey[]>([]);

  const [activeCredentialTab, setActiveCredentialTab] =
    useState<ProviderCredentialTab>(providerAccess.canViewAccounts ? "accounts" : "officialKeys");

  const [providerGroupsVisible, setProviderGroupsVisible] = useState(false);

  const [activeAccountProvider, setActiveAccountProvider] = useState<AccountProviderKey>("gpt");

  const [accountQuotaTarget, setAccountQuotaTarget] = useState<GptAccount | null>(null);

  const [accountQuotaResponse, setAccountQuotaResponse] =
    useState<GptAccountQuotaResult | null>(null);

  const [accountQuotaLoading, setAccountQuotaLoading] = useState(false);

  const [accountQuotaError, setAccountQuotaError] = useState<string | null>(null);

  const [rateLimitResetTarget, setRateLimitResetTarget] = useState<GptAccount | null>(null);

  const [rateLimitResetResponse, setRateLimitResetResponse] =
    useState<RateLimitResetCreditsResponse | null>(null);

  const [rateLimitResetLoading, setRateLimitResetLoading] = useState(false);

  const [rateLimitResetError, setRateLimitResetError] = useState<string | null>(null);

  const [applyingResetCreditId, setApplyingResetCreditId] = useState<string | null>(null);

  const [unassignedProviderResources, setUnassignedProviderResources] = useState<
    UnassignedProviderResource[]
  >([]);

  const [gptAccountsPage, setGptAccountsPage] = useState<ListPageState>(initialListPageState);

  const [claudeAccountsPage, setClaudeAccountsPage] = useState<ListPageState>(initialListPageState);

  const [gptUpstreamApiKeysPage, setGptUpstreamApiKeysPage] =
    useState<ListPageState>(initialListPageState);

  const [claudeUpstreamApiKeysPage, setClaudeUpstreamApiKeysPage] =
    useState<ListPageState>(initialListPageState);

  const [requestOverrideTarget, setRequestOverrideTarget] = useState<RequestOverrideTarget | null>(null);

  const [loading, setLoading] = useState(true);

  const [unassignedProviderResourcesLoading, setUnassignedProviderResourcesLoading] =
    useState(false);

  const [saving, setSaving] = useState(false);

  const [providerGroupSavingId, setProviderGroupSavingId] = useState<string | null>(null);

  const [requestOverrideSaving, setRequestOverrideSaving] = useState(false);

  const [oauthLoading, setOauthLoading] = useState(false);

  const [enabledUpdatingId, setEnabledUpdatingId] = useState<string | null>(null);

  const [accountDeletingId, setAccountDeletingId] = useState<string | null>(null);

  const [upstreamApiKeyDeletingId, setUpstreamApiKeyDeletingId] = useState<string | null>(null);

  const [upstreamApiKeyEnabledUpdatingId, setUpstreamApiKeyEnabledUpdatingId] =
    useState<string | null>(null);

  const [resourceGroupUpdatingId, setResourceGroupUpdatingId] = useState<string | null>(null);

  const [accountImportOpen, setAccountImportOpen] = useState(false);

  const [accountImportMode, setAccountImportMode] = useState<AccountImportMode>("oauth");

  const [providerGroupCreateProvider, setProviderGroupCreateProvider] =
    useState<UpstreamApiKeyProvider | null>(null);

  const [providerGroupModelsTarget, setProviderGroupModelsTarget] =
    useState<ProviderGroupSummary | null>(null);

  const [upstreamApiKeyDialogProvider, setUpstreamApiKeyDialogProvider] =
    useState<UpstreamApiKeyProvider | null>(null);

  const [authorization, setAuthorization] = useState<OauthAuthorizationResponse | null>(null);

  const activeAccountProviderMeta =
    accountProviderTabs.find((provider) => provider.key === activeAccountProvider) ?? accountProviderTabs[0];

  const activeCredentialPage = activeCredentialTab === "officialKeys"
    ? activeAccountProvider === "claude"
      ? claudeUpstreamApiKeysPage
      : gptUpstreamApiKeysPage
    : activeAccountProvider === "claude"
      ? claudeAccountsPage
      : gptAccountsPage;

  async function loadAccounts(offsets: Partial<AccountPageOffsets> = {}) {
    const token = authToken;
    const request = beginRequest("resources");
    const provider = asUpstreamApiKeyProvider(activeAccountProvider);
    if (providerGroupsVisible || !provider) {
      setLoading(false);
      return;
    }
    const official = activeCredentialTab === "officialKeys";
    if ((official && !providerAccess.canViewOfficialApiKeys) || (!official && !providerAccess.canViewAccounts)) {
      setLoading(false);
      return;
    }
    setLoading(true);
    const init = { signal: request.signal };
    try {
      if (official) {
        const offset = provider === "claude" ? offsets.claudeUpstreamKeys ?? claudeUpstreamApiKeysPage.offset : offsets.gptUpstreamKeys ?? gptUpstreamApiKeysPage.offset;
        const data = await requestJson<ListProviderUpstreamApiKeysResponse>(listPagePath(upstreamApiKeyPath(provider), offset), init, token);
        if (!request.isCurrent()) return;
        if (provider === "claude") { setClaudeUpstreamApiKeys(data.items); setClaudeUpstreamApiKeysPage(pageStateFrom(data)); }
        else { setGptUpstreamApiKeys(data.items); setGptUpstreamApiKeysPage(pageStateFrom(data)); }
      } else if (provider === "claude") {
        const data = await requestJson<ListClaudeAccountsResponse>(listPagePath(claudeAccountsPath, offsets.claude ?? claudeAccountsPage.offset), init, token);
        if (!request.isCurrent()) return;
        setClaudeAccounts(data.items); setClaudeAccountsPage(pageStateFrom(data));
      } else {
        const data = await requestJson<ListAccountsResponse>(listPagePath(gptAccountsPath, offsets.gpt ?? gptAccountsPage.offset), init, token);
        if (!request.isCurrent()) return;
        setAccounts(data.items); setGptAccountsPage(pageStateFrom(data));
      }
    } catch (error) {
      if (request.isCurrent()) showErrorToast("资源加载失败", error);
    } finally {
      if (request.isCurrent()) setLoading(false);
    }
  }

  async function loadActiveCredentialPage(offset: number) {
    if (activeCredentialTab === "officialKeys") {
      await loadAccounts(
        activeAccountProvider === "claude"
          ? { claudeUpstreamKeys: offset }
          : { gptUpstreamKeys: offset },
      );
      return;
    }
    if (activeAccountProvider === "claude") {
      await loadAccounts({ claude: offset });
      return;
    }
    if (activeAccountProvider === "gpt") {
      await loadAccounts({ gpt: offset });
    }
  }

  function openAccountImportDialog(mode: AccountImportMode = "oauth") {
    setAccountImportMode(activeAccountProvider === "claude" ? "oauth" : mode);
    setAuthorization(null);
    setAccountImportOpen(true);
  }

  function closeAccountImportDialog() {
    if (saving || oauthLoading) {
      return;
    }
    setAccountImportOpen(false);
  }

  function showProviderGroups() {
    const provider = asUpstreamApiKeyProvider(activeAccountProvider);
    if (provider) {
      setProviderGroupsVisible(true);
    }
  }

  async function openProviderGroupCreateDialog() {
    const provider = asUpstreamApiKeyProvider(activeAccountProvider);
    const token = authToken;
    if (!provider || !token) {
      return;
    }
    setProviderGroupCreateProvider(provider);
    setUnassignedProviderResources([]);
    setUnassignedProviderResourcesLoading(true);
    const request = beginRequest("unassigned-resources");
    try {
      const params = new URLSearchParams({ provider });
      const resources = await requestJson<UnassignedProviderResource[]>(
        `${providerGroupsPath}/unassigned-resources?${params.toString()}`,
        { signal: request.signal },
        token,
      );
      if (request.isCurrent()) {
        setUnassignedProviderResources(resources);
      }
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("未分组资源加载失败", error);
      }
    } finally {
      if (request.isCurrent()) {
        setUnassignedProviderResourcesLoading(false);
      }
    }
  }

  function closeProviderGroupCreateDialog() {
    if (!providerGroupSavingId) {
      beginRequest("unassigned-resources");
      setProviderGroupCreateProvider(null);
      setUnassignedProviderResources([]);
      setUnassignedProviderResourcesLoading(false);
    }
  }

  function openUpstreamApiKeyDialog() {
    const provider = asUpstreamApiKeyProvider(activeAccountProvider);
    if (!provider) {
      return;
    }
    setUpstreamApiKeyDialogProvider(provider);
  }

  function closeUpstreamApiKeyDialog() {
    if (saving) {
      return;
    }
    setUpstreamApiKeyDialogProvider(null);
  }

  function updateUpstreamApiKeyList(
    provider: UpstreamApiKeyProvider,
    update: (items: ProviderUpstreamApiKey[]) => ProviderUpstreamApiKey[],
  ) {
    if (provider === "claude") {
      setClaudeUpstreamApiKeys(update);
    } else {
      setGptUpstreamApiKeys(update);
    }
  }

  function openRequestOverrideDialog(target: RequestOverrideTarget) {
    if (!target.item.override) {
      toast.error("请求覆盖不可见", { description: "当前用户没有查看该资源覆盖配置的权限。" });
      return;
    }
    setRequestOverrideTarget(target);
  }

  function closeRequestOverrideDialog() {
    if (requestOverrideSaving) {
      return;
    }
    setRequestOverrideTarget(null);
  }

  function closeProviderGroupModelsDialog() {
    if (providerGroupModelsTarget?.id === providerGroupSavingId) {
      return;
    }
    setProviderGroupModelsTarget(null);
  }

  async function createProviderGroup(
    provider: UpstreamApiKeyProvider,
    name: string,
    models: string[],
    accountIds: string[],
    apiKeyIds: string[],
  ): Promise<boolean> {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return false;
    }
    if (utf8ByteLength(name) > 128) {
      toast.error("Provider 分组创建失败", { description: "分组名称不能超过 128 字节。" });
      return false;
    }
    if (models.length === 0) {
      toast.error("Provider 分组创建失败", { description: "至少需要配置一个限制模型。" });
      return false;
    }
    if (models.length > 128 || models.some((model) => utf8ByteLength(model) > 256)) {
      toast.error("Provider 分组创建失败", {
        description: "限制模型最多 128 项，每个模型名最多 256 字节。",
      });
      return false;
    }

    setProviderGroupSavingId("create");
    try {
      await requestJson<ProviderGroup>(providerGroupsPath, {
        method: "POST",
        body: JSON.stringify({
          provider,
          name,
          models,
          account_ids: accountIds,
          api_key_ids: apiKeyIds,
        }),
      }, token);
      await Promise.all([loadProviderGroups(), loadAccounts()]);
      if (isActiveAuthToken(authToken)) toast.success(`${providerLabel(provider)} 分组已创建`);
      return true;
    } catch (error) {
      showErrorToast("Provider 分组创建失败", error);
      if (isProviderStateSyncError(error)) {
        // 分组和资源归属已经提交，关闭创建弹窗并重新读取数据库事实，避免重复提交。
        setProviderGroupCreateProvider(null);
        setUnassignedProviderResources([]);
        await Promise.all([loadProviderGroups(), loadAccounts()]);
      }
      return false;
    } finally {
      void reloadResourceUsage();
      setProviderGroupSavingId(null);
    }
  }

  async function renameProviderGroup(group: ProviderGroupSummary, name: string): Promise<boolean> {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return false;
    }
    if (utf8ByteLength(name) > 128) {
      toast.error("Provider 分组重命名失败", { description: "分组名称不能超过 128 字节。" });
      return false;
    }

    setProviderGroupSavingId(group.id);
    try {
      await requestJson<ProviderGroup>(`${providerGroupsPath}/${group.id}`, {
        method: "PUT",
        body: JSON.stringify({ name }),
      }, token);
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("Provider 分组名称已更新");
      return true;
    } catch (error) {
      showErrorToast("Provider 分组重命名失败", error);
      return false;
    } finally {
      setProviderGroupSavingId(null);
    }
  }

  async function toggleProviderGroupEnabled(group: ProviderGroupSummary): Promise<boolean> {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return false;
    }

    setProviderGroupSavingId(group.id);
    try {
      await requestJson<ProviderGroup>(`${providerGroupsPath}/${group.id}/enabled`, {
        method: "POST",
        body: JSON.stringify({ enabled: !group.enabled }),
      }, token);
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success(group.enabled ? "Provider 分组已停用" : "Provider 分组已恢复");
      return true;
    } catch (error) {
      showErrorToast(group.enabled ? "Provider 分组停用失败" : "Provider 分组恢复失败", error);
      return false;
    } finally {
      setProviderGroupSavingId(null);
    }
  }

  function requestDeleteProviderGroup(group: ProviderGroupSummary) {
    setConfirmationRequest({
      title: `删除 ${providerLabel(group.provider)} 分组`,
      description: `将删除分组“${group.name}”。受影响资源：${group.counts.account_count} 个 OAuth 账号和 ${group.counts.upstream_api_key_count} 个上游官方 Key 将解除分组、退出调度但保留凭证；${group.counts.gateway_api_key_count} 个调用方网关 Key 将保留原分组绑定并失效，后续请求返回 401。历史请求日志不受影响。`,
      confirmLabel: "删除分组",
      pendingLabel: "正在删除",
      onConfirm: () => deleteProviderGroup(group),
    });
  }

  async function deleteProviderGroup(group: ProviderGroupSummary) {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return;
    }

    setProviderGroupSavingId(group.id);
    try {
      const deleted = await requestJson<DeleteProviderGroupResponse>(`${providerGroupsPath}/${group.id}`, {
        method: "DELETE",
      }, token);
      if (!isActiveAuthToken(token)) return;
      await Promise.all([loadProviderGroups(), loadAccounts()]);
      if (isActiveAuthToken(authToken)) toast.success("Provider 分组已删除", {
        description: `${deleted.affected_gateway_api_key_count} 个网关 Key 已保留，分组绑定失效。`,
      });
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("Provider 分组删除失败", error);
        if (isProviderStateSyncError(error)) {
          await Promise.all([loadProviderGroups(), loadAccounts()]);
        }
      }
    } finally {
      void reloadResourceUsage();
      if (isActiveAuthToken(token)) setProviderGroupSavingId(null);
    }
  }

  async function updateProviderGroupModels(
    group: ProviderGroupSummary,
    models: string[],
  ): Promise<boolean> {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return false;
    }
    if (models.length === 0 || models.length > 128) {
      toast.error("Provider 分组模型更新失败", {
        description: "模型白名单必须包含 1 到 128 项。",
      });
      return false;
    }
    if (models.some((model) => utf8ByteLength(model.trim()) > 256)) {
      toast.error("Provider 分组模型更新失败", {
        description: "每个模型名最多 256 字节。",
      });
      return false;
    }

    setProviderGroupSavingId(group.id);
    try {
      await requestJson<ProviderGroup>(`${providerGroupsPath}/${group.id}/models`, {
        method: "PUT",
        body: JSON.stringify({ models }),
      }, token);
      if (!isActiveAuthToken(token)) return false;
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("Provider 分组模型白名单已更新");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("Provider 分组模型更新失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setProviderGroupSavingId(null);
    }
  }

  async function createAuthorization() {
    setOauthLoading(true);
    try {
      const isClaude = activeAccountProvider === "claude";
      const data = await requestJson<OauthAuthorizationResponse>(
        isClaude ? `${claudeAccountsPath}/oauth/authorize` : `${gptAccountsPath}/oauth/authorize`,
        {
          method: "POST",
        },
        authToken,
      );
      setAuthorization(data);
      if (isActiveAuthToken(authToken)) toast.success("授权链接已生成", {
        description: isClaude
          ? "授权后请复制页面显示的 authorization code。"
          : "授权后请复制浏览器地址栏中的 callback URL。",
      });
    } catch (error) {
      showErrorToast("授权链接生成失败", error);
    } finally {
      void reloadResourceUsage();
      setOauthLoading(false);
    }
  }

  async function submitCallback(callbackUrl: string) {
    const isClaude = activeAccountProvider === "claude";
    if (utf8ByteLength(callbackUrl.trim()) > 16 * 1024) {
      toast.error("OAuth 账号导入失败", { description: "授权结果不能超过 16384 字节。" });
      return;
    }
    setSaving(true);
    try {
      await requestJson<GptAccount | ClaudeAccount>(isClaude ? `${claudeAccountsPath}/oauth/callback` : `${gptAccountsPath}/oauth/callback`, {
        method: "POST",
        body: JSON.stringify(
          isClaude
            ? {
              authorization_result: callbackUrl,
              state: authorization?.state,
            }
            : { callback_url: callbackUrl },
        ),
      }, authToken);
      setAuthorization(null);
      setAccountImportOpen(false);
      if (isActiveAuthToken(authToken)) toast.success(`${isClaude ? "Claude" : "GPT"} OAuth 账号已导入`);
      await Promise.all([
        loadAccounts(isClaude ? { claude: 0 } : { gpt: 0 }),
        loadProviderGroups(),
      ]);
    } catch (error) {
      showErrorToast("OAuth 账号导入失败", error);
      if (isProviderStateSyncError(error)) {
        setAuthorization(null);
        setAccountImportOpen(false);
        await Promise.all([
          loadAccounts(isClaude ? { claude: 0 } : { gpt: 0 }),
          loadProviderGroups(),
        ]);
      }
    } finally {
      void reloadResourceUsage();
      setSaving(false);
    }
  }

  async function submitManualAccount({ refreshToken, clientId, chatgptAccountId }: { refreshToken: string; clientId: string; chatgptAccountId: string }) {
    if (
      utf8ByteLength(refreshToken.trim()) > 32 * 1024 ||
      utf8ByteLength(clientId.trim()) > 512 ||
      utf8ByteLength(chatgptAccountId.trim()) > 512
    ) {
      toast.error("账号保存失败", { description: "凭证字段超过允许长度，请检查粘贴内容。" });
      return;
    }
    setSaving(true);
    try {
      await requestJson<GptAccount>(gptAccountsPath, {
        method: "POST",
        body: JSON.stringify({
          refresh_token: refreshToken.trim(),
          client_id: clientId.trim() || undefined,
          chatgpt_account_id: chatgptAccountId.trim() || undefined,
        }),
      }, authToken);
      setAccountImportOpen(false);
      if (isActiveAuthToken(authToken)) toast.success("账号已保存");
      await Promise.all([loadAccounts({ gpt: 0 }), loadProviderGroups()]);
    } catch (error) {
      showErrorToast("账号保存失败", error);
      if (isProviderStateSyncError(error)) {
        setAccountImportOpen(false);
        await Promise.all([loadAccounts({ gpt: 0 }), loadProviderGroups()]);
      }
    } finally {
      void reloadResourceUsage();
      setSaving(false);
    }
  }

  async function submitUpstreamApiKey({ apiKey: officialApiKey, baseUrl: officialBaseUrl }: ProviderUpstreamApiKeyDialogInput) {
    const provider = upstreamApiKeyDialogProvider;
    if (!provider) {
      return;
    }
    const normalizedApiKey = officialApiKey.trim();
    const normalizedBaseUrl = officialBaseUrl.trim();
    if (
      !isAscii(normalizedApiKey) ||
      utf8ByteLength(normalizedApiKey) > 4 * 1024 ||
      utf8ByteLength(normalizedBaseUrl) > 2 * 1024
    ) {
      toast.error("官方 Key 保存失败", {
        description: "API Key 必须是最多 4096 字节的 ASCII；Base URL 最多 2048 字节。",
      });
      return;
    }
    setSaving(true);
    try {
      await requestJson<ProviderUpstreamApiKey>(
        upstreamApiKeyPath(provider),
        {
          method: "POST",
          body: JSON.stringify({
            api_key: normalizedApiKey,
            base_url: normalizedBaseUrl,
            override: { header: {}, body: {} },
          }),
        },
        authToken,
      );
      await Promise.all([
        loadAccounts(
          provider === "claude" ? { claudeUpstreamKeys: 0 } : { gptUpstreamKeys: 0 },
        ),
        loadProviderGroups(),
      ]);
      setUpstreamApiKeyDialogProvider(null);
      if (isActiveAuthToken(authToken)) toast.success(`${providerLabel(provider)} 官方 Key 已保存`);
    } catch (error) {
      showErrorToast("官方 Key 保存失败", error);
      if (isProviderStateSyncError(error)) {
        // 创建已经落库，销毁凭证输入并重新读取列表，不能让用户在原表单上再次提交。
        setUpstreamApiKeyDialogProvider(null);
        await Promise.all([
          loadAccounts(
            provider === "claude" ? { claudeUpstreamKeys: 0 } : { gptUpstreamKeys: 0 },
          ),
          loadProviderGroups(),
        ]);
      }
    } finally {
      void reloadResourceUsage();
      setSaving(false);
    }
  }

  async function submitRequestOverride({ headerRows: requestOverrideHeaderRows, bodyRows: requestOverrideBodyRows }: { headerRows: OverrideEntry[]; bodyRows: OverrideEntry[] }) {
    const token = authToken;
    const target = requestOverrideTarget;
    if (!token || !target || !canUpdateRequestOverride(target)) {
      return;
    }

    let requestOverride: RequestOverride;
    try {
      requestOverride = {
        header: overrideEntriesToObject(requestOverrideHeaderRows, "header"),
        body: overrideEntriesToObject(requestOverrideBodyRows, "body"),
      };
    } catch (error) {
      showErrorToast("请求覆盖保存失败", error);
      return;
    }

    setRequestOverrideSaving(true);
    try {
      const basePath = target.kind === "account"
        ? gptAccountsPath
        : target.kind === "claudeAccount"
          ? claudeAccountsPath
          : upstreamApiKeyPath(target.provider);
      const saved = await requestJson<GptAccount | ClaudeAccount | ProviderUpstreamApiKey>(
        `${basePath}/${target.item.id}/override`,
        {
          method: "PUT",
          body: JSON.stringify({ override: requestOverride }),
        },
        token,
      );
      if (!isActiveAuthToken(token)) {
        return;
      }
      if (target.kind === "account") {
        const account = saved as GptAccount;
        setAccounts((items) => items.map((item) => (item.id === account.id ? account : item)));
      } else if (target.kind === "claudeAccount") {
        const account = saved as ClaudeAccount;
        setClaudeAccounts((items) => items.map((item) => (item.id === account.id ? account : item)));
      } else {
        const apiKey = saved as ProviderUpstreamApiKey;
        updateUpstreamApiKeyList(target.provider, (items) =>
          items.map((item) => (item.id === apiKey.id ? apiKey : item)),
        );
      }
      setRequestOverrideTarget(null);
      if (isActiveAuthToken(authToken)) toast.success("请求覆盖已保存");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("请求覆盖保存失败", error);
        if (isProviderStateSyncError(error)) {
          setRequestOverrideTarget(null);
          await loadAccounts();
        }
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setRequestOverrideSaving(false);
      }
    }
  }

  function canUpdateRequestOverride(target: RequestOverrideTarget) {
    if (target.kind === "apiKey") {
      return providerAccess.has(target.item.group?.id, "official_api_key.override.update");
    }
    return providerAccess.has(target.item.group?.id, "account.override.update");
  }

  async function updateGptAccountGroup(account: GptAccount, groupId: string) {
    if (groupId === (account.group?.id ?? "")) {
      return;
    }
    setResourceGroupUpdatingId(account.id);
    try {
      const updated = await requestJson<GptAccount>(`${gptAccountsPath}/${account.id}/group`, {
        method: "PUT",
        body: JSON.stringify({ group_id: groupId || null }),
      }, authToken);
      setAccounts((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("GPT 账号分组已更新");
    } catch (error) {
      showErrorToast("GPT 账号分组更新失败", error);
      if (isProviderStateSyncError(error)) {
        await Promise.all([loadAccounts(), loadProviderGroups()]);
      }
    } finally {
      setResourceGroupUpdatingId(null);
    }
  }

  async function updateClaudeAccountGroup(account: ClaudeAccount, groupId: string) {
    if (groupId === (account.group?.id ?? "")) {
      return;
    }
    setResourceGroupUpdatingId(account.id);
    try {
      const updated = await requestJson<ClaudeAccount>(`${claudeAccountsPath}/${account.id}/group`, {
        method: "PUT",
        body: JSON.stringify({ group_id: groupId || null }),
      }, authToken);
      setClaudeAccounts((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("Claude 账号分组已更新");
    } catch (error) {
      showErrorToast("Claude 账号分组更新失败", error);
      if (isProviderStateSyncError(error)) {
        await Promise.all([loadAccounts(), loadProviderGroups()]);
      }
    } finally {
      setResourceGroupUpdatingId(null);
    }
  }

  async function updateUpstreamApiKeyGroup(
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
    groupId: string,
  ) {
    if (groupId === (apiKey.group?.id ?? "")) {
      return;
    }
    setResourceGroupUpdatingId(apiKey.id);
    try {
      const updated = await requestJson<ProviderUpstreamApiKey>(
        `${upstreamApiKeyPath(provider)}/${apiKey.id}/group`,
        {
          method: "PUT",
          body: JSON.stringify({ group_id: groupId || null }),
        },
        authToken,
      );
      updateUpstreamApiKeyList(provider, (items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success(`${providerLabel(provider)} 官方 Key 分组已更新`);
    } catch (error) {
      showErrorToast("官方 Key 分组更新失败", error);
      if (isProviderStateSyncError(error)) {
        await Promise.all([loadAccounts(), loadProviderGroups()]);
      }
    } finally {
      setResourceGroupUpdatingId(null);
    }
  }

  async function updateEnabled(account: GptAccount, enabled: boolean) {
    setEnabledUpdatingId(account.id);
    try {
      const updated = await requestJson<GptAccount>(`${gptAccountsPath}/${account.id}/enabled`, {
        method: "PUT",
        body: JSON.stringify({ enabled }),
      }, authToken);
      setAccounts((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      if (isActiveAuthToken(authToken)) toast.success("账号调度开关已更新");
    } catch (error) {
      showErrorToast("账号调度开关更新失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      setEnabledUpdatingId(null);
    }
  }

  async function updateClaudeEnabled(account: ClaudeAccount, enabled: boolean) {
    setEnabledUpdatingId(account.id);
    try {
      const updated = await requestJson<ClaudeAccount>(`${claudeAccountsPath}/${account.id}/enabled`, {
        method: "PUT",
        body: JSON.stringify({ enabled }),
      }, authToken);
      setClaudeAccounts((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      if (isActiveAuthToken(authToken)) toast.success("Claude 账号调度开关已更新");
    } catch (error) {
      showErrorToast("Claude 账号调度开关更新失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      setEnabledUpdatingId(null);
    }
  }

  async function updateUpstreamApiKeyEnabled(
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
    enabled: boolean,
  ) {
    setUpstreamApiKeyEnabledUpdatingId(apiKey.id);
    try {
      const updated = await requestJson<ProviderUpstreamApiKey>(`${upstreamApiKeyPath(provider)}/${apiKey.id}/enabled`, {
        method: "PUT",
        body: JSON.stringify({ enabled }),
      }, authToken);
      updateUpstreamApiKeyList(provider, (items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      if (isActiveAuthToken(authToken)) toast.success("官方 Key 调度开关已更新");
    } catch (error) {
      showErrorToast("官方 Key 调度开关更新失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      setUpstreamApiKeyEnabledUpdatingId(null);
    }
  }

  async function fetchAccountQuota(account: GptAccount) {
    const quotaEndpoint = currentUser?.role === "tenant_owner" ? "quota-with-usage" : "quota";
    const quota = await requestJson<GptAccountQuotaResult>(
      `${gptAccountsPath}/${account.id}/${quotaEndpoint}`,
      {
        method: "POST",
      },
      authToken,
    );
    if (quota.quota_limit_removed) {
      // 后端已同时更新 PostgreSQL quota 状态与 Redis 调度投影，重新加载账号列表，避免
      // 页面继续展示查询前的 quota_limited 快照。
      await loadAccounts();
    }
    return quota;
  }

  async function openAccountQuotaDialog(account: GptAccount) {
    setAccountQuotaTarget(account);
    setAccountQuotaResponse(null);
    setAccountQuotaError(null);
    setAccountQuotaLoading(true);
    try {
      const quota = await fetchAccountQuota(account);
      setAccountQuotaResponse(quota);
      if (quota.quota_limit_removed) {
        if (isActiveAuthToken(authToken)) toast.success("账号额度查询成功，额度限制已解除");
      } else {
        if (isActiveAuthToken(authToken)) toast.success("账号额度查询成功");
      }
    } catch (error) {
      console.error("[dashboard] 账号额度查询失败", error);
      setAccountQuotaError(errorMessageFrom(error));
    } finally {
      setAccountQuotaLoading(false);
    }
  }

  function closeAccountQuotaDialog() {
    if (accountQuotaLoading) {
      return;
    }
    setAccountQuotaTarget(null);
    setAccountQuotaResponse(null);
    setAccountQuotaError(null);
  }

  async function fetchRateLimitResetCredits(account: GptAccount) {
    return requestJson<RateLimitResetCreditsResponse>(
      `${gptAccountsPath}/${account.id}/rate-limit-reset-credits`,
      { method: "GET" },
      authToken,
    );
  }

  async function openRateLimitResetDialog(account: GptAccount) {
    setRateLimitResetTarget(account);
    setRateLimitResetResponse(null);
    setRateLimitResetError(null);
    setRateLimitResetLoading(true);
    try {
      setRateLimitResetResponse(await fetchRateLimitResetCredits(account));
    } catch (error) {
      setRateLimitResetError(errorMessageFrom(error));
    } finally {
      setRateLimitResetLoading(false);
    }
  }

  async function applyRateLimitResetCredit(credit: RateLimitResetCredit) {
    if (
      !rateLimitResetTarget ||
      !providerAccess.has(rateLimitResetTarget.group?.id, "account.reset.consume") ||
      applyingResetCreditId ||
      rateLimitResetLoading
    ) {
      return;
    }

    const account = rateLimitResetTarget;
    setApplyingResetCreditId(credit.id);
    setRateLimitResetError(null);
    try {
      const result = await requestJson<ConsumeRateLimitResetCreditResponse>(
        `${gptAccountsPath}/${account.id}/rate-limit-reset-credits/consume`,
        {
          method: "POST",
          body: JSON.stringify({
            credit_id: credit.id,
          }),
        },
        authToken,
      );

      switch (result.code) {
        case "reset":
          if (isActiveAuthToken(authToken)) toast.success(
            result.windows_reset > 0
              ? `额度重置已应用，共重置 ${result.windows_reset} 个窗口`
              : "额度重置已应用",
          );
          break;
        case "already_redeemed":
          if (isActiveAuthToken(authToken)) toast.success("该次额度重置此前已成功应用");
          break;
        case "nothing_to_reset":
          toast.info("当前没有符合条件的额度窗口可以重置");
          break;
        case "no_credit":
          toast.error("账号当前没有可用的额度重置次数");
          break;
      }

      // 无论上游返回何种业务结果都重新读取列表；成功或幂等成功时还要同步额度与网关
      // quota_limited 状态，避免账号已恢复但调度快照仍不可用。
      setRateLimitResetLoading(true);
      try {
        const [creditsResult, quotaResult] = await Promise.allSettled([
          fetchRateLimitResetCredits(account),
          result.code === "reset" || result.code === "already_redeemed"
            ? fetchAccountQuota(account)
            : Promise.resolve(null),
        ]);
        if (creditsResult.status === "fulfilled") {
          setRateLimitResetResponse(creditsResult.value);
        } else {
          setRateLimitResetError(
            `额度重置操作已完成，但重新查询列表失败：${errorMessageFrom(creditsResult.reason)}`,
          );
        }
        if (quotaResult.status === "rejected") {
          showErrorToast("额度重置已应用，但账号额度同步失败", quotaResult.reason);
        }
      } finally {
        setRateLimitResetLoading(false);
      }
    } catch (error) {
      showErrorToast("额度重置应用失败", error);
    } finally {
      setApplyingResetCreditId(null);
    }
  }

  function closeRateLimitResetDialog() {
    if (rateLimitResetLoading || applyingResetCreditId) {
      return;
    }
    setRateLimitResetTarget(null);
    setRateLimitResetResponse(null);
    setRateLimitResetError(null);
  }

  function requestDeleteAccount(account: GptAccount) {
    const accountLabel = account.email || account.account_id || account.id;
    setConfirmationRequest({
      title: "删除 GPT 账号",
      description: `将删除 GPT 账号“${accountLabel}”，并移除数据库凭证和调度运行态。`,
      confirmLabel: "删除账号",
      pendingLabel: "正在删除",
      onConfirm: () => deleteAccount(account),
    });
  }

  async function deleteAccount(account: GptAccount) {
    setAccountDeletingId(account.id);
    try {
      const deleted = await requestJson<DeleteGptAccountResponse>(`${gptAccountsPath}/${account.id}`, {
        method: "DELETE",
      }, authToken);
      setAccounts((items) => items.filter((item) => item.id !== deleted.id));
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("账号已删除");
    } catch (error) {
      showErrorToast("账号删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      void reloadResourceUsage();
      setAccountDeletingId(null);
    }
  }

  function requestDeleteClaudeAccount(account: ClaudeAccount) {
    const accountLabel = account.email || account.account_uuid || account.id;
    setConfirmationRequest({
      title: "删除 Claude 账号",
      description: `将删除 Claude 账号“${accountLabel}”，并移除数据库凭证和调度运行态。`,
      confirmLabel: "删除账号",
      pendingLabel: "正在删除",
      onConfirm: () => deleteClaudeAccount(account),
    });
  }

  async function deleteClaudeAccount(account: ClaudeAccount) {
    setAccountDeletingId(account.id);
    try {
      const deleted = await requestJson<DeleteClaudeAccountResponse>(`${claudeAccountsPath}/${account.id}`, {
        method: "DELETE",
      }, authToken);
      setClaudeAccounts((items) => items.filter((item) => item.id !== deleted.id));
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("Claude 账号已删除");
    } catch (error) {
      showErrorToast("Claude 账号删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      void reloadResourceUsage();
      setAccountDeletingId(null);
    }
  }

  function requestDeleteUpstreamApiKey(
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
  ) {
    setConfirmationRequest({
      title: `删除 ${providerLabel(provider)} 官方 Key`,
      description: `将删除官方 Key“${apiKey.masked_api_key}”，并移除数据库凭证和调度运行态。`,
      confirmLabel: "删除官方 Key",
      pendingLabel: "正在删除",
      onConfirm: () => deleteUpstreamApiKey(provider, apiKey),
    });
  }

  async function deleteUpstreamApiKey(
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
  ) {
    setUpstreamApiKeyDeletingId(apiKey.id);
    try {
      const deleted = await requestJson<DeleteProviderUpstreamApiKeyResponse>(`${upstreamApiKeyPath(provider)}/${apiKey.id}`, {
        method: "DELETE",
      }, authToken);
      updateUpstreamApiKeyList(provider, (items) =>
        items.filter((item) => item.id !== deleted.id),
      );
      await loadProviderGroups();
      if (isActiveAuthToken(authToken)) toast.success("官方 Key 已删除");
    } catch (error) {
      showErrorToast("官方 Key 删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      void reloadResourceUsage();
      setUpstreamApiKeyDeletingId(null);
    }
  }

  async function copyAuthorizationUrl() {
    if (!authorization) {
      return;
    }
    try {
      await navigator.clipboard.writeText(authorization.authorization_url);
      if (isActiveAuthToken(authToken)) toast.success("授权链接已复制");
    } catch (error) {
      showErrorToast("授权链接复制失败", error);
    }
  }

  const resetOperationAccountId =
    rateLimitResetTarget && (rateLimitResetLoading || applyingResetCreditId)
      ? rateLimitResetTarget.id
      : null;

  const quotaOperationAccountId =
    accountQuotaTarget && accountQuotaLoading ? accountQuotaTarget.id : null;
  useEffect(() => {
    if (!providerAccess.canViewAccounts && providerAccess.canViewOfficialApiKeys) setActiveCredentialTab("officialKeys");
    else if (providerAccess.canViewAccounts && !providerAccess.canViewOfficialApiKeys) setActiveCredentialTab("accounts");
  }, [providerAccess.canViewAccounts, providerAccess.canViewOfficialApiKeys]);
  useEffect(() => { void loadAccounts(); }, [refreshRevision, activeAccountProvider, activeCredentialTab, providerGroupsVisible, providerAccess]);
  useEffect(() => { if (currentUser.role === "tenant_owner") void loadProviderGroups(); }, [refreshRevision]);
  useEffect(() => { onLoadingChange(providerGroupsVisible ? providerGroupsLoading : loading); return () => onLoadingChange(false); }, [providerGroupsVisible, providerGroupsLoading, loading, onLoadingChange]);
  return <>
    {providerAccess.isOwner && <div className="mb-4 space-y-1 text-sm text-slate-500" role="status">
      {resourceUsageLoading ? "正在加载资源与分组总量…" : !resourceUsage ? "资源与分组总量加载失败，请刷新页面。" : <>
        <p>{`上游资源总数：${resourceUsage.resource_count} / ${resourceUsage.max_resources ?? "不限制"}。跨所有 Provider 合计账号和官方 API Key；停用、失效和未分组资源也计数，删除才释放名额。${resourceLimitReached ? "已达到上限，无法新增资源。" : ""}`}</p>
        <p>{`分组总数：${resourceUsage.provider_group_count} / ${resourceUsage.max_provider_groups ?? "不限制"}。跨所有 Provider 合计；停用和空分组也计数，删除才释放名额。${groupLimitReached ? "已达到上限，无法新增分组。" : ""}`}</p>
      </>}
    </div>}
    <ProvidersPage
    resourceCreationDisabled={resourceUsageLoading || !resourceUsage || resourceLimitReached}
    groupCreationDisabled={resourceUsageLoading || !resourceUsage || groupLimitReached}
    access={providerAccess}
    accounts={accounts}
    claudeAccounts={claudeAccounts}
    gptUpstreamApiKeys={gptUpstreamApiKeys}
    claudeUpstreamApiKeys={claudeUpstreamApiKeys}
    loading={loading}
    activeProvider={activeAccountProvider}
    activeCredentialTab={activeCredentialTab}
    providerGroupsVisible={providerGroupsVisible}
    providerGroupsLoading={providerGroupsLoading}
    providerGroupSavingId={providerGroupSavingId}
    enabledUpdatingId={enabledUpdatingId}
    accountDeletingId={accountDeletingId}
    upstreamApiKeyDeletingId={upstreamApiKeyDeletingId}
    upstreamApiKeyEnabledUpdatingId={upstreamApiKeyEnabledUpdatingId}
    quotaOperationAccountId={quotaOperationAccountId}
    resetOperationAccountId={resetOperationAccountId}
    providerGroups={providerGroups}
    resourceGroupUpdatingId={resourceGroupUpdatingId}
    pageOffset={activeCredentialPage.offset}
    pageSize={dashboardListPageSize}
    nextPageOffset={activeCredentialPage.nextOffset}
    onProviderChange={(provider) => {
      setActiveAccountProvider(provider);
      setActiveCredentialTab(providerAccess.canViewAccounts ? "accounts" : "officialKeys");
      setProviderGroupsVisible(false);
    }}
    onCredentialTabChange={(tab) => {
      setActiveCredentialTab(tab);
      setProviderGroupsVisible(false);
    }}
    onProviderGroupsView={showProviderGroups}
    onOpenAccountImport={openAccountImportDialog}
    onOpenUpstreamApiKey={openUpstreamApiKeyDialog}
    onOpenProviderGroupCreate={openProviderGroupCreateDialog}
    onRenameProviderGroup={renameProviderGroup}
    onEditProviderGroupModels={setProviderGroupModelsTarget}
    onToggleProviderGroupEnabled={toggleProviderGroupEnabled}
    onDeleteProviderGroup={requestDeleteProviderGroup}
    onUpdateClaudeGroup={updateClaudeAccountGroup}
    onUpdateGptGroup={updateGptAccountGroup}
    onUpdateUpstreamApiKeyGroup={updateUpstreamApiKeyGroup}
    onUpdateClaudeEnabled={updateClaudeEnabled}
    onUpdateGptEnabled={updateEnabled}
    onUpdateUpstreamApiKeyEnabled={updateUpstreamApiKeyEnabled}
    onOpenAccountQuota={openAccountQuotaDialog}
    onOpenRateLimitReset={openRateLimitResetDialog}
    onDeleteGptAccount={requestDeleteAccount}
    onDeleteClaudeAccount={requestDeleteClaudeAccount}
    onDeleteUpstreamApiKey={requestDeleteUpstreamApiKey}
    onOpenRequestOverride={openRequestOverrideDialog}
    onPageChange={loadActiveCredentialPage}
  /><AnimatePresence>{accountImportOpen && (
    <AccountImportDialog
      key={`account-import-${activeAccountProvider}`}
      provider={activeAccountProvider}
      providerLabel={activeAccountProviderMeta.label}
      initialMode={accountImportMode}
      authorization={authorization}
      saving={saving}
      oauthLoading={oauthLoading}
      onClose={closeAccountImportDialog}
      onCreateAuthorization={createAuthorization}
      onCopyAuthorizationUrl={copyAuthorizationUrl}
      onSubmitCallback={submitCallback}
      onSubmitManual={submitManualAccount}
    />
  )}
      {accountQuotaTarget && (
        <AccountQuotaDialog
          key={`account-quota-${accountQuotaTarget.id}`}
          account={accountQuotaTarget}
          response={accountQuotaResponse}
          loading={accountQuotaLoading}
          error={accountQuotaError}
          onClose={closeAccountQuotaDialog}
        />
      )}
      {rateLimitResetTarget && (
        <RateLimitResetDialog
          key={`rate-limit-reset-${rateLimitResetTarget.id}`}
          account={rateLimitResetTarget}
          response={rateLimitResetResponse}
          loading={rateLimitResetLoading}
          error={rateLimitResetError}
          applyingCreditId={applyingResetCreditId}
          canConsume={providerAccess.has(
            rateLimitResetTarget.group?.id,
            "account.reset.consume",
          )}
          onApply={applyRateLimitResetCredit}
          onClose={closeRateLimitResetDialog}
        />
      )}
      {providerGroupCreateProvider && (
        <ProviderGroupCreateDialog
          key="provider-group-create"
          providerLabel={providerLabel(providerGroupCreateProvider)}
          saving={providerGroupSavingId === "create"}
          resourcesLoading={unassignedProviderResourcesLoading}
          resources={unassignedProviderResources}
          onCreate={(name, models, accountIds, apiKeyIds) =>
            createProviderGroup(
              providerGroupCreateProvider,
              name,
              models,
              accountIds,
              apiKeyIds,
            )
          }
          onClose={closeProviderGroupCreateDialog}
        />
      )}
      {providerGroupModelsTarget && (
        <ModelWhitelistDialog
          key={`provider-group-models-${providerGroupModelsTarget.id}`}
          titleId="providerGroupModelsTitle"
          title="修改分组模型"
          description={`修改 Provider 分组“${providerGroupModelsTarget.name}”的模型白名单。`}
          models={providerGroupModelsTarget.allowed_models}
          saving={providerGroupSavingId === providerGroupModelsTarget.id}
          onSave={(models) => updateProviderGroupModels(providerGroupModelsTarget, models)}
          onClose={closeProviderGroupModelsDialog}
        />
      )}
      {upstreamApiKeyDialogProvider && (
        <ProviderUpstreamApiKeyDialog
          key={`upstream-api-key-${upstreamApiKeyDialogProvider}`}
          providerLabel={providerLabel(upstreamApiKeyDialogProvider)}
          baseUrlPlaceholder={defaultUpstreamApiKeyBaseUrl(upstreamApiKeyDialogProvider)}
          saving={saving}
          onSubmit={submitUpstreamApiKey}
          onClose={closeUpstreamApiKeyDialog}
        />
      )}
      {requestOverrideTarget && (
        <RequestOverrideDialog
          key={`request-override-${requestOverrideTarget.kind}-${requestOverrideTarget.item.id}`}
          target={requestOverrideTarget}
          saving={requestOverrideSaving}
          readOnly={!canUpdateRequestOverride(requestOverrideTarget)}
          onSubmit={submitRequestOverride}
          onClose={closeRequestOverrideDialog}
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
