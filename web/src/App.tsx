import { FormEvent, lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence } from "motion/react";
import { toast } from "sonner";
import {
  isDashboardAuthError,
  isProviderStateSyncError,
  requestFormData,
  requestJson,
  setAuthExpiredHandler,
} from "./api/client";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { DashboardShell } from "./components/DashboardShell";
import { ModelWhitelistDialog } from "./components/ModelWhitelistDialog";
import {
  accountProviderTabs,
  apiKeysPath,
  authTokenStorageKey,
  claudeAccountsPath,
  claudeUpstreamApiKeysPath,
  dashboardListPageSize,
  dashboardRoutes,
  defaultGptClientId,
  gptAccountsPath,
  gptUpstreamApiKeysPath,
  maxUserConcurrency,
  maxUserQuota,
  pluginsPath,
  providerGroupsPath,
  requestLogPageSize,
  requestLogsPath,
  usagePath,
  usersPath,
  themeStorageKey,
} from "./config";
import { AccountImportDialog } from "./features/accounts/AccountImportDialog";
import { ProviderGroupCreateDialog } from "./features/accounts/ProviderGroupCreateDialog";
import { ProviderUpstreamApiKeyDialog } from "./features/accounts/ProviderUpstreamApiKeyDialog";
import { RateLimitResetDialog } from "./features/accounts/RateLimitResetDialog";
import { RequestOverrideDialog } from "./features/accounts/RequestOverrideDialog";
import { AuthScreen } from "./features/auth/AuthScreen";
import {
  createOverrideEntry,
  overrideEntriesFromObject,
  overrideEntriesToObject,
} from "./features/accounts/utils";
import { ApiKeyCreateDialog } from "./features/api-keys/ApiKeyCreateDialog";
import { ApiKeyPluginDialog } from "./features/api-keys/ApiKeyPluginDialog";
import type { PluginArtifactFiles } from "./features/plugins/PluginArtifactFileFields";
import {
  PluginCreateDialog,
  type CreatePluginInput,
} from "./features/plugins/PluginCreateDialog";
import { PluginReleaseDialog } from "./features/plugins/PluginReleaseDialog";
import { requestLogAutoLoadKey } from "./features/request-logs/utils";
import { UserConcurrencyDialog } from "./features/users/UserConcurrencyDialog";
import { UserCreateDialog } from "./features/users/UserCreateDialog";
import { UserQuotaDialog } from "./features/users/UserQuotaDialog";
import { errorMessageFrom, showErrorToast } from "./lib/errors";
import {
  activePageLoading,
  normalizeDashboardPath,
  pageFromPath,
  routesForUser,
} from "./lib/routing";
import { shiftDateInputValue, todayInputValue } from "./lib/format";
import { AccountsPage } from "./pages/AccountsPage";
import { ApiKeysPage } from "./pages/ApiKeysPage";
import { PluginsPage } from "./pages/PluginsPage";
import { RequestLogsPage } from "./pages/RequestLogsPage";
import { TenantsPage } from "./pages/TenantsPage";
import { UsersPage } from "./pages/UsersPage";
import type {
  AccountImportMode,
  AccountProviderKey,
  ApiKey,
  AuthResponse,
  ClaudeAccount,
  ConsumeRateLimitResetCreditResponse,
  DashboardTenant,
  DashboardUser,
  DashboardUserListItem,
  DashboardTheme,
  DeleteApiKeyResponse,
  DeleteClaudeAccountResponse,
  DeleteGptAccountResponse,
  DeletePluginResponse,
  DeleteProviderUpstreamApiKeyResponse,
  DeleteProviderGroupResponse,
  GptAccount,
  GptAccountQuotaResponse,
  ProviderCredentialTab,
  ProviderGroup,
  ProviderGroupSummary,
  ProviderUpstreamApiKey,
  ListAccountsResponse,
  ListApiKeysResponse,
  ListClaudeAccountsResponse,
  ListProviderUpstreamApiKeysResponse,
  ListRequestLogsResponse,
  ListUsersResponse,
  MeResponse,
  OauthAuthorizationResponse,
  OverrideEntry,
  PluginReleaseSummary,
  RateLimitResetCredit,
  RateLimitResetCreditsResponse,
  RequestLogCursor,
  RequestLogRecord,
  RequestOverride,
  RequestOverrideTarget,
  UpstreamApiKeyProvider,
  UsageResponse,
  UnassignedProviderResource,
} from "./types";

interface ConfirmationRequest {
  title: string;
  description: string;
  confirmLabel: string;
  pendingLabel: string;
  onConfirm: () => Promise<void>;
}

// 图表库只在普通用户进入用量概览时加载，管理员管理面板不承担 ECharts 资源开销。
const UsagePage = lazy(() =>
  import("./pages/UsagePage").then((module) => ({ default: module.UsagePage })),
);

const usageLoadErrorToastId = "usage-load-error";

export function App() {
  const [currentPath, setCurrentPath] = useState(() => normalizeDashboardPath(window.location.pathname));
  const [authToken, setAuthToken] = useState(() => localStorage.getItem(authTokenStorageKey));
  const [currentUser, setCurrentUser] = useState<DashboardUser | null>(null);
  const [currentTenant, setCurrentTenant] = useState<DashboardTenant | null>(null);
  const [serviceTimezone, setServiceTimezone] = useState("UTC");
  const [requestLogRetentionDays, setRequestLogRetentionDays] = useState(30);
  const [theme, setTheme] = useState<DashboardTheme>(() =>
    localStorage.getItem(themeStorageKey) === "dark" ? "dark" : "light",
  );
  const [authLoading, setAuthLoading] = useState(true);
  const [authSubmitting, setAuthSubmitting] = useState(false);
  const [authMode, setAuthMode] = useState<"login" | "register">("login");
  const [loginIdentifier, setLoginIdentifier] = useState("");
  const [loginPassword, setLoginPassword] = useState("");
  const [registerUsername, setRegisterUsername] = useState("");
  const [registerTenantCode, setRegisterTenantCode] = useState("");
  const [registerEmail, setRegisterEmail] = useState("");
  const [registerPassword, setRegisterPassword] = useState("");
  const [registerCode, setRegisterCode] = useState("");
  const [emailCodeSending, setEmailCodeSending] = useState(false);
  const [accounts, setAccounts] = useState<GptAccount[]>([]);
  const [claudeAccounts, setClaudeAccounts] = useState<ClaudeAccount[]>([]);
  const [gptUpstreamApiKeys, setGptUpstreamApiKeys] = useState<ProviderUpstreamApiKey[]>([]);
  const [claudeUpstreamApiKeys, setClaudeUpstreamApiKeys] =
    useState<ProviderUpstreamApiKey[]>([]);
  const [activeCredentialTab, setActiveCredentialTab] =
    useState<ProviderCredentialTab>("accounts");
  const [providerGroupsVisible, setProviderGroupsVisible] = useState(false);
  const [activeAccountProvider, setActiveAccountProvider] = useState<AccountProviderKey>("gpt");
  const [users, setUsers] = useState<DashboardUserListItem[]>([]);
  const [accountQuotas, setAccountQuotas] = useState<Record<string, GptAccountQuotaResponse>>({});
  const [rateLimitResetTarget, setRateLimitResetTarget] = useState<GptAccount | null>(null);
  const [rateLimitResetResponse, setRateLimitResetResponse] =
    useState<RateLimitResetCreditsResponse | null>(null);
  const [rateLimitResetLoading, setRateLimitResetLoading] = useState(false);
  const [rateLimitResetError, setRateLimitResetError] = useState<string | null>(null);
  const [applyingResetCreditId, setApplyingResetCreditId] = useState<string | null>(null);
  const [apiKeys, setApiKeys] = useState<ApiKey[]>([]);
  const [plugins, setPlugins] = useState<PluginReleaseSummary[]>([]);
  const [pluginOptions, setPluginOptions] = useState<PluginReleaseSummary[]>([]);
  const [providerGroups, setProviderGroups] = useState<ProviderGroupSummary[]>([]);
  const [providerGroupOptions, setProviderGroupOptions] = useState<ProviderGroup[]>([]);
  const [unassignedProviderResources, setUnassignedProviderResources] = useState<
    UnassignedProviderResource[]
  >([]);
  const [gptAccountsPage, setGptAccountsPage] = useState<ListPageState>(initialListPageState);
  const [claudeAccountsPage, setClaudeAccountsPage] = useState<ListPageState>(initialListPageState);
  const [gptUpstreamApiKeysPage, setGptUpstreamApiKeysPage] =
    useState<ListPageState>(initialListPageState);
  const [claudeUpstreamApiKeysPage, setClaudeUpstreamApiKeysPage] =
    useState<ListPageState>(initialListPageState);
  const [usersPage, setUsersPage] = useState<ListPageState>(initialListPageState);
  const [apiKeysPage, setApiKeysPage] = useState<ListPageState>(initialListPageState);
  const [requestOverrideTarget, setRequestOverrideTarget] = useState<RequestOverrideTarget | null>(null);
  const [requestOverrideHeaderRows, setRequestOverrideHeaderRows] = useState<OverrideEntry[]>([]);
  const [requestOverrideBodyRows, setRequestOverrideBodyRows] = useState<OverrideEntry[]>([]);
  const [requestLogs, setRequestLogs] = useState<RequestLogRecord[]>([]);
  const [requestLogDate, setRequestLogDate] = useState(() => todayInputValue("UTC"));
  const [requestLogNonSuccessOnly, setRequestLogNonSuccessOnly] = useState(false);
  const [requestLogNextCursor, setRequestLogNextCursor] = useState<RequestLogCursor | null>(null);
  const [requestLogCursorStack, setRequestLogCursorStack] = useState<Array<RequestLogCursor | null>>([]);
  const [requestLogCurrentCursor, setRequestLogCurrentCursor] = useState<RequestLogCursor | null>(null);
  const [requestLogAutoLoadedKey, setRequestLogAutoLoadedKey] = useState<string | null>(null);
  const [usage, setUsage] = useState<UsageResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [usersLoading, setUsersLoading] = useState(false);
  const [apiKeysLoading, setApiKeysLoading] = useState(true);
  const [pluginsLoading, setPluginsLoading] = useState(false);
  const [providerGroupsLoading, setProviderGroupsLoading] = useState(false);
  const [unassignedProviderResourcesLoading, setUnassignedProviderResourcesLoading] =
    useState(false);
  const [requestLogsLoading, setRequestLogsLoading] = useState(false);
  const [usageLoading, setUsageLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [apiKeySaving, setApiKeySaving] = useState(false);
  const [pluginSavingId, setPluginSavingId] = useState<string | null>(null);
  const [providerGroupSavingId, setProviderGroupSavingId] = useState<string | null>(null);
  const [requestOverrideSaving, setRequestOverrideSaving] = useState(false);
  const [oauthLoading, setOauthLoading] = useState(false);
  const [enabledUpdatingId, setEnabledUpdatingId] = useState<string | null>(null);
  const [userUpdatingId, setUserUpdatingId] = useState<string | null>(null);
  const [accountDeletingId, setAccountDeletingId] = useState<string | null>(null);
  const [upstreamApiKeyDeletingId, setUpstreamApiKeyDeletingId] = useState<string | null>(null);
  const [upstreamApiKeyEnabledUpdatingId, setUpstreamApiKeyEnabledUpdatingId] =
    useState<string | null>(null);
  const [resourceGroupUpdatingId, setResourceGroupUpdatingId] = useState<string | null>(null);
  const [quotaRefreshingIds, setQuotaRefreshingIds] = useState<Record<string, boolean>>({});
  const [apiKeyUpdatingId, setApiKeyUpdatingId] = useState<string | null>(null);
  const [accountImportOpen, setAccountImportOpen] = useState(false);
  const [accountImportMode, setAccountImportMode] = useState<AccountImportMode>("oauth");
  const [providerGroupCreateProvider, setProviderGroupCreateProvider] =
    useState<UpstreamApiKeyProvider | null>(null);
  const [providerGroupModelsTarget, setProviderGroupModelsTarget] =
    useState<ProviderGroupSummary | null>(null);
  const [upstreamApiKeyDialogProvider, setUpstreamApiKeyDialogProvider] =
    useState<UpstreamApiKeyProvider | null>(null);
  const [apiKeyCreateOpen, setApiKeyCreateOpen] = useState(false);
  const [apiKeyModelsTarget, setApiKeyModelsTarget] = useState<ApiKey | null>(null);
  const [apiKeyPluginTarget, setApiKeyPluginTarget] = useState<ApiKey | null>(null);
  const [apiKeyPluginReleaseId, setApiKeyPluginReleaseId] = useState("");
  const [apiKeyPluginSaving, setApiKeyPluginSaving] = useState(false);
  const [pluginCreateOpen, setPluginCreateOpen] = useState(false);
  const [pluginReleaseTarget, setPluginReleaseTarget] = useState<PluginReleaseSummary | null>(null);
  const [userQuotaDialogUser, setUserQuotaDialogUser] = useState<DashboardUser | null>(null);
  const [userQuotaValue, setUserQuotaValue] = useState("");
  const [userConcurrencyDialogUser, setUserConcurrencyDialogUser] =
    useState<DashboardUser | null>(null);
  const [userConcurrencyValue, setUserConcurrencyValue] = useState("");
  const [userCreateOpen, setUserCreateOpen] = useState(false);
  const [userCreateUsername, setUserCreateUsername] = useState("");
  const [userCreateEmail, setUserCreateEmail] = useState("");
  const [userCreatePassword, setUserCreatePassword] = useState("");
  const [userCreating, setUserCreating] = useState(false);
  const [authorization, setAuthorization] = useState<OauthAuthorizationResponse | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");
  const [refreshToken, setRefreshToken] = useState("");
  const [clientId, setClientId] = useState(defaultGptClientId);
  const [chatgptAccountId, setChatgptAccountId] = useState("");
  const [officialApiKey, setOfficialApiKey] = useState("");
  const [officialBaseUrl, setOfficialBaseUrl] = useState("https://api.openai.com/v1");
  const [apiKeyName, setApiKeyName] = useState("");
  const [apiKeyAllowedModels, setApiKeyAllowedModels] = useState<string[]>([]);
  const [selectedPluginReleaseId, setSelectedPluginReleaseId] = useState("");
  const [selectedProviderGroupId, setSelectedProviderGroupId] = useState("");
  const [confirmationRequest, setConfirmationRequest] = useState<ConfirmationRequest | null>(null);
  const [confirmationSubmitting, setConfirmationSubmitting] = useState(false);
  const [tenantRefreshSignal, setTenantRefreshSignal] = useState(0);
  const usageAutoLoadedKeyRef = useRef<string | null>(null);
  const usageRequestSequenceRef = useRef(0);
  const unassignedProviderResourcesRequestSequenceRef = useRef(0);

  const activeAccountProviderMeta =
    accountProviderTabs.find((provider) => provider.key === activeAccountProvider) ?? accountProviderTabs[0];
  const visibleRoutes = useMemo(() => routesForUser(currentUser), [currentUser]);
  const activePage = pageFromPath(currentPath, visibleRoutes);
  const activeRoute = visibleRoutes.find((route) => route.page === activePage) ?? visibleRoutes[0] ?? dashboardRoutes[0];
  const activeCredentialPage = activeCredentialTab === "officialKeys"
    ? activeAccountProvider === "claude"
      ? claudeUpstreamApiKeysPage
      : gptUpstreamApiKeysPage
    : activeAccountProvider === "claude"
      ? claudeAccountsPage
      : gptAccountsPage;

  useEffect(() => {
    // 刷新页面时 currentUser 会先短暂为 null。如果此时按普通用户路由归一化，管理员专属
    // 地址会先被改写为 /dashboard/usage，待身份恢复后又回退到 /admin/accounts。必须等
    // /dash/auth/me 完成后再按真实角色校验 URL，才能正确保留刷新前的管理员页面。
    if (authLoading) {
      return;
    }

    const normalized = normalizeDashboardPath(window.location.pathname, visibleRoutes);
    if (normalized !== window.location.pathname) {
      window.history.replaceState(null, "", normalized);
    }
    setCurrentPath(normalized);

    function handlePopState() {
      setCurrentPath(normalizeDashboardPath(window.location.pathname, visibleRoutes));
    }

    window.addEventListener("popstate", handlePopState);
    return () => window.removeEventListener("popstate", handlePopState);
  }, [authLoading, visibleRoutes]);

  useEffect(() => {
    setAuthExpiredHandler((token, error) => expireDashboardSession(token, error));
    return () => setAuthExpiredHandler(null);
  }, []);

  useEffect(() => {
    void loadCurrentUser();
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    document.documentElement.style.colorScheme = theme;
    localStorage.setItem(themeStorageKey, theme);
  }, [theme]);

  useEffect(() => {
    if (!currentUser || !authToken) {
      return;
    }

    if (currentUser.role === "tenant_owner") {
      void loadAccounts({ gpt: 0, claude: 0, gptUpstreamKeys: 0, claudeUpstreamKeys: 0 });
      void loadUsers(0);
      void loadProviderGroups();
      void loadPlugins();
      void loadApiKeys(0);
      void loadPluginOptions();
    } else if (currentUser.role === "tenant_user") {
      setLoading(false);
      setUsersLoading(false);
      void loadProviderGroupOptions();
      void loadApiKeys(0);
      void loadPluginOptions();
    } else {
      setLoading(false);
      setUsersLoading(false);
      setApiKeysLoading(false);
    }
  }, [currentUser, authToken]);

  useEffect(() => {
    if (activePage !== "usage" || !currentUser || !authToken) {
      return;
    }

    // 固定年度窗口每天会向前滚动一天；把服务时区日期纳入缓存键，跨午夜后重新进入页面时
    // 自动请求新的 365 天窗口，而不是继续复用上一自然日的结果。
    const queryKey = `${currentUser.id}:${todayInputValue(serviceTimezone)}`;
    if (usageAutoLoadedKeyRef.current === queryKey) {
      return;
    }
    usageAutoLoadedKeyRef.current = queryKey;
    void loadUsage();
  }, [activePage, authToken, currentUser, serviceTimezone]);

  useEffect(() => {
    if (activePage !== "requestLogs" || !currentUser || !authToken) {
      return;
    }

    const queryKey = requestLogAutoLoadKey(currentUser.id, requestLogDate, requestLogNonSuccessOnly);
    if (requestLogAutoLoadedKey === queryKey) {
      return;
    }

    // 同一个用户、日期和筛选条件只自动加载一次。失败后只能由手动刷新或筛选变化再次请求，
    // 避免请求结束后的状态更新形成隐式重试循环。
    setRequestLogAutoLoadedKey(queryKey);
    void loadRequestLogs(null, []);
  }, [activePage, authToken, currentUser, requestLogDate, requestLogNonSuccessOnly, requestLogAutoLoadedKey]);

  useEffect(() => {
    resetRequestLogPaging();
  }, [requestLogDate, requestLogNonSuccessOnly]);

  async function loadCurrentUser() {
    if (!authToken) {
      setAuthLoading(false);
      return;
    }

    const token = authToken;
    setAuthLoading(true);
    try {
      const data = await requestJson<MeResponse>("/dash/auth/me", undefined, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setCurrentUser(data.user);
      setCurrentTenant(data.tenant);
      setServiceTimezone(data.service_timezone);
      setRequestLogRetentionDays(data.request_log_retention_days);
      setRequestLogDate(todayInputValue(data.service_timezone));
    } catch (error) {
      if (isDashboardAuthError(error)) {
        expireDashboardSession(token, error);
      } else if (isActiveAuthToken(token)) {
        // 临时数据库/Redis 故障不代表 JWT 失效，保留 token 以便用户直接重试或重新登录。
        showErrorToast("登录状态检查失败", error);
      }
    } finally {
      setAuthLoading(false);
    }
  }

  function applyAuth(data: AuthResponse) {
    resetDashboardState();
    resetAuthInputs();
    localStorage.setItem(authTokenStorageKey, data.token);
    setAuthToken(data.token);
    setCurrentUser(data.user);
    setCurrentTenant(data.tenant);
    setServiceTimezone(data.service_timezone);
    setRequestLogRetentionDays(data.request_log_retention_days);
    setRequestLogDate(todayInputValue(data.service_timezone));
    setCurrentPath(normalizeDashboardPath(window.location.pathname, routesForUser(data.user)));
  }

  async function submitLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (utf8ByteLength(loginPassword) > 72) {
      toast.error("登录失败", { description: "密码 UTF-8 编码长度不能超过 72 字节。" });
      return;
    }
    setAuthSubmitting(true);
    try {
      const data = await requestJson<AuthResponse>("/dash/auth/login", {
        method: "POST",
        body: JSON.stringify({
          identifier: loginIdentifier.trim(),
          password: loginPassword,
        }),
      });
      applyAuth(data);
      toast.success("登录成功");
    } catch (error) {
      if (isDashboardAuthError(error)) {
        // 登录接口没有携带既有 Dashboard token，因此不会触发全局会话失效 Toast；这里
        // 单独展示统一凭证错误，同时不区分邮箱不存在、密码错误或用户被禁用。
        console.error("[dashboard] 登录凭证校验失败", error);
        toast.error("登录失败", { description: "邮箱、用户名或密码错误。" });
      } else {
        showErrorToast("登录失败", error);
      }
    } finally {
      setAuthSubmitting(false);
    }
  }

  async function sendRegisterEmailCode() {
    setEmailCodeSending(true);
    try {
      await requestJson<{ status: string }>("/dash/auth/register/email-code", {
        method: "POST",
        body: JSON.stringify({ email: registerEmail.trim() }),
      });
      toast.success("验证码已发送");
    } catch (error) {
      showErrorToast("验证码发送失败", error);
    } finally {
      setEmailCodeSending(false);
    }
  }

  async function submitRegister(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const username = registerUsername.trim().toLowerCase();
    const usernameCharacters = Array.from(username);
    if (
      usernameCharacters.length === 0 ||
      usernameCharacters.length > 32 ||
      utf8ByteLength(username) > 128 ||
      !/^[\p{L}\p{N}][\p{L}\p{N}_-]*$/u.test(username)
    ) {
      toast.error("注册失败", {
        description: "用户名最多 32 个字符，只能包含字母、数字、下划线和连字符。",
      });
      return;
    }
    if (Array.from(registerPassword).length < 8 || utf8ByteLength(registerPassword) > 72) {
      toast.error("注册失败", {
        description: "密码至少 8 个字符，且 UTF-8 编码长度不能超过 72 字节。",
      });
      return;
    }
    setAuthSubmitting(true);
    try {
      const data = await requestJson<AuthResponse>("/dash/auth/register", {
        method: "POST",
        body: JSON.stringify({
          username,
          tenant_code: registerTenantCode.trim(),
          email: registerEmail.trim(),
          password: registerPassword,
          email_code: registerCode.trim(),
        }),
      });
      applyAuth(data);
      toast.success("注册成功");
    } catch (error) {
      showErrorToast("注册失败", error);
    } finally {
      setAuthSubmitting(false);
    }
  }

  function logout() {
    localStorage.removeItem(authTokenStorageKey);
    setAuthToken(null);
    setCurrentUser(null);
    setCurrentTenant(null);
    setServiceTimezone("UTC");
    setRequestLogRetentionDays(30);
    setRequestLogDate(todayInputValue("UTC"));
    resetDashboardState();
    resetAuthInputs();
  }

  function expireDashboardSession(token: string, error: unknown) {
    if (!isActiveAuthToken(token)) {
      return;
    }
    localStorage.removeItem(authTokenStorageKey);
    setAuthToken(null);
    setCurrentUser(null);
    setCurrentTenant(null);
    setServiceTimezone("UTC");
    setRequestLogRetentionDays(30);
    setRequestLogDate(todayInputValue("UTC"));
    resetDashboardState();
    resetAuthInputs();
    toast.error("登录状态已失效", {
      description: errorMessageFrom(error),
    });
  }

  function resetAuthInputs() {
    setAuthMode("login");
    setLoginIdentifier("");
    setLoginPassword("");
    setRegisterUsername("");
    setRegisterTenantCode("");
    setRegisterEmail("");
    setRegisterPassword("");
    setRegisterCode("");
    setEmailCodeSending(false);
  }

  function resetDashboardState() {
    setAccounts([]);
    setClaudeAccounts([]);
    setGptUpstreamApiKeys([]);
    setClaudeUpstreamApiKeys([]);
    setActiveCredentialTab("accounts");
    setProviderGroupsVisible(false);
    setActiveAccountProvider("gpt");
    setUsers([]);
    setAccountQuotas({});
    setApiKeys([]);
    setPlugins([]);
    setPluginOptions([]);
    setProviderGroups([]);
    setProviderGroupOptions([]);
    setGptAccountsPage(initialListPageState());
    setClaudeAccountsPage(initialListPageState());
    setGptUpstreamApiKeysPage(initialListPageState());
    setClaudeUpstreamApiKeysPage(initialListPageState());
    setUsersPage(initialListPageState());
    setApiKeysPage(initialListPageState());
    setRequestOverrideTarget(null);
    setRequestOverrideHeaderRows([]);
    setRequestOverrideBodyRows([]);
    setRequestLogs([]);
    setRequestLogAutoLoadedKey(null);
    setRequestLogNextCursor(null);
    setRequestLogCursorStack([]);
    setRequestLogCurrentCursor(null);
    setUsage(null);
    usageAutoLoadedKeyRef.current = null;
    usageRequestSequenceRef.current += 1;
    unassignedProviderResourcesRequestSequenceRef.current += 1;
    setConfirmationRequest(null);
    setConfirmationSubmitting(false);
    setApiKeyName("");
    setApiKeyAllowedModels([]);
    setSelectedPluginReleaseId("");
    setSelectedProviderGroupId("");
    setApiKeyUpdatingId(null);
    setAccountImportOpen(false);
    setAccountImportMode("oauth");
    setProviderGroupCreateProvider(null);
    setProviderGroupModelsTarget(null);
    setUnassignedProviderResources([]);
    setUpstreamApiKeyDialogProvider(null);
    setApiKeyCreateOpen(false);
    setApiKeyModelsTarget(null);
    setApiKeyPluginTarget(null);
    setApiKeyPluginReleaseId("");
    setApiKeyPluginSaving(false);
    setPluginCreateOpen(false);
    setPluginReleaseTarget(null);
    setUserQuotaDialogUser(null);
    setUserQuotaValue("");
    setUserCreateOpen(false);
    setUserCreateUsername("");
    setUserCreateEmail("");
    setUserCreatePassword("");
    setUserCreating(false);
    setAuthorization(null);
    setCallbackUrl("");
    setRefreshToken("");
    setClientId(defaultGptClientId);
    setChatgptAccountId("");
    setOfficialApiKey("");
    setOfficialBaseUrl("https://api.openai.com/v1");
    setAccountDeletingId(null);
    setUpstreamApiKeyDeletingId(null);
    setEnabledUpdatingId(null);
    setUpstreamApiKeyEnabledUpdatingId(null);
    setResourceGroupUpdatingId(null);
    setProviderGroupSavingId(null);
    setUserUpdatingId(null);
    setQuotaRefreshingIds({});
    setLoading(false);
    setUsersLoading(false);
    setApiKeysLoading(false);
    setPluginsLoading(false);
    setProviderGroupsLoading(false);
    setUnassignedProviderResourcesLoading(false);
    setRequestLogsLoading(false);
    setUsageLoading(false);
    setSaving(false);
    setApiKeySaving(false);
    setPluginSavingId(null);
    setRequestOverrideSaving(false);
    setOauthLoading(false);
  }

  function isActiveAuthToken(token: string | null | undefined) {
    return Boolean(token) && localStorage.getItem(authTokenStorageKey) === token;
  }

  async function loadAccounts(offsets: Partial<AccountPageOffsets> = {}) {
    const token = authToken;
    if (!token) {
      setAccounts([]);
      setClaudeAccounts([]);
      setGptUpstreamApiKeys([]);
      setClaudeUpstreamApiKeys([]);
      setGptAccountsPage(initialListPageState());
      setClaudeAccountsPage(initialListPageState());
      setGptUpstreamApiKeysPage(initialListPageState());
      setClaudeUpstreamApiKeysPage(initialListPageState());
      setLoading(false);
      return;
    }

    const gptOffset = offsets.gpt ?? gptAccountsPage.offset;
    const claudeOffset = offsets.claude ?? claudeAccountsPage.offset;
    const gptUpstreamKeysOffset = offsets.gptUpstreamKeys ?? gptUpstreamApiKeysPage.offset;
    const claudeUpstreamKeysOffset =
      offsets.claudeUpstreamKeys ?? claudeUpstreamApiKeysPage.offset;
    setLoading(true);
    try {
      const [data, gptUpstreamKeys, claudeData, claudeUpstreamKeys] = await Promise.all([
        requestJson<ListAccountsResponse>(listPagePath(gptAccountsPath, gptOffset), undefined, token),
        requestJson<ListProviderUpstreamApiKeysResponse>(
          listPagePath(gptUpstreamApiKeysPath, gptUpstreamKeysOffset),
          undefined,
          token,
        ),
        requestJson<ListClaudeAccountsResponse>(
          listPagePath(claudeAccountsPath, claudeOffset),
          undefined,
          token,
        ),
        requestJson<ListProviderUpstreamApiKeysResponse>(
          listPagePath(claudeUpstreamApiKeysPath, claudeUpstreamKeysOffset),
          undefined,
          token,
        ),
      ]);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setAccounts(data.items);
      setGptUpstreamApiKeys(gptUpstreamKeys.items);
      setClaudeAccounts(claudeData.items);
      setClaudeUpstreamApiKeys(claudeUpstreamKeys.items);
      setGptAccountsPage(pageStateFrom(data));
      setGptUpstreamApiKeysPage(pageStateFrom(gptUpstreamKeys));
      setClaudeAccountsPage(pageStateFrom(claudeData));
      setClaudeUpstreamApiKeysPage(pageStateFrom(claudeUpstreamKeys));
      const visibleAccountIds = new Set(data.items.map((account) => account.id));
      setAccountQuotas((items) =>
        Object.fromEntries(Object.entries(items).filter(([accountId]) => visibleAccountIds.has(accountId))),
      );
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("账号加载失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setLoading(false);
      }
    }
  }

  /** 管理员列表包含停用分组和依赖统计，同时更新所有创建弹窗使用的启用选项。 */
  async function loadProviderGroups() {
    const token = authToken;
    if (!token) {
      setProviderGroups([]);
      setProviderGroupOptions([]);
      setProviderGroupsLoading(false);
      return;
    }

    setProviderGroupsLoading(true);
    try {
      const groups = await requestJson<ProviderGroupSummary[]>(providerGroupsPath, undefined, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setProviderGroups(groups);
      setProviderGroupOptions(groups.filter((group) => group.enabled));
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("Provider 分组加载失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setProviderGroupsLoading(false);
      }
    }
  }

  /** 普通用户只读取可用于创建网关 Key 的启用分组，不暴露管理统计。 */
  async function loadProviderGroupOptions() {
    const token = authToken;
    if (!token) {
      setProviderGroupOptions([]);
      return;
    }
    try {
      const groups = await requestJson<ProviderGroup[]>(`${providerGroupsPath}/options`, undefined, token);
      if (isActiveAuthToken(token)) {
        setProviderGroupOptions(groups);
      }
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("Provider 分组选项加载失败", error);
      }
    }
  }

  async function loadUsage() {
    const token = authToken;
    if (!token || !currentUser) {
      setUsage(null);
      setUsageLoading(false);
      return;
    }

    const requestSequence = ++usageRequestSequenceRef.current;
    setUsageLoading(true);
    try {
      const data = await requestJson<UsageResponse>(
        usagePath,
        undefined,
        token,
      );
      if (!isActiveAuthToken(token) || requestSequence !== usageRequestSequenceRef.current) {
        return;
      }
      setUsage(data);
    } catch (error) {
      if (isActiveAuthToken(token) && requestSequence === usageRequestSequenceRef.current) {
        showErrorToast("用量数据加载失败", error, usageLoadErrorToastId);
      }
    } finally {
      if (isActiveAuthToken(token) && requestSequence === usageRequestSequenceRef.current) {
        setUsageLoading(false);
      }
    }
  }

  async function loadUsers(offset = usersPage.offset) {
    const token = authToken;
    if (!token) {
      setUsers([]);
      setUsersPage(initialListPageState());
      setUsersLoading(false);
      return;
    }

    setUsersLoading(true);
    try {
      const data = await requestJson<ListUsersResponse>(listPagePath(usersPath, offset), undefined, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setUsers(data.items);
      setUsersPage(pageStateFrom(data));
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("用户加载失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setUsersLoading(false);
      }
    }
  }

  async function loadApiKeys(offset = apiKeysPage.offset) {
    const token = authToken;
    if (!token) {
      setApiKeys([]);
      setApiKeysPage(initialListPageState());
      setApiKeysLoading(false);
      return;
    }

    setApiKeysLoading(true);
    try {
      const data = await requestJson<ListApiKeysResponse>(listPagePath(apiKeysPath, offset), undefined, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeys(data.items);
      setApiKeysPage(pageStateFrom(data));
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("API Key 加载失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setApiKeysLoading(false);
      }
    }
  }

  async function loadPlugins() {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      setPlugins([]);
      setPluginsLoading(false);
      return;
    }

    setPluginsLoading(true);
    try {
      const data = await requestJson<PluginReleaseSummary[]>(pluginsPath, undefined, token);
      if (isActiveAuthToken(token)) setPlugins(data);
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件加载失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginsLoading(false);
    }
  }

  async function loadPluginOptions() {
    const token = authToken;
    if (!token) {
      setPluginOptions([]);
      return;
    }
    try {
      const data = await requestJson<PluginReleaseSummary[]>(
        `${pluginsPath}/options`,
        undefined,
        token,
      );
      if (isActiveAuthToken(token)) setPluginOptions(data);
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件选项加载失败", error);
    }
  }

  async function loadRequestLogs(
    cursor: RequestLogCursor | null = null,
    cursorStack: Array<RequestLogCursor | null> = requestLogCursorStack,
  ) {
    const token = authToken;
    if (!token) {
      resetRequestLogPaging();
      setRequestLogsLoading(false);
      return;
    }

    setRequestLogsLoading(true);
    try {
      const data = await requestJson<ListRequestLogsResponse>(
        `${requestLogsPath}?${requestLogQueryParams(cursor).toString()}`,
        undefined,
        token,
      );
      if (!isActiveAuthToken(token)) {
        return;
      }
      setRequestLogs(data.items);
      setRequestLogCurrentCursor(cursor);
      setRequestLogCursorStack(cursorStack);
      setRequestLogNextCursor(data.next_cursor);
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("请求日志加载失败", error);
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setRequestLogsLoading(false);
      }
    }
  }

  function resetRequestLogPaging() {
    setRequestLogs([]);
    setRequestLogNextCursor(null);
    setRequestLogCursorStack([]);
    setRequestLogCurrentCursor(null);
  }

  async function loadNextRequestLogPage() {
    if (!requestLogNextCursor) {
      return;
    }

    const nextStack = [...requestLogCursorStack, requestLogCurrentCursor];
    await loadRequestLogs(requestLogNextCursor, nextStack);
  }

  async function loadPreviousRequestLogPage() {
    if (requestLogCursorStack.length === 0) {
      return;
    }

    const nextStack = requestLogCursorStack.slice(0, -1);
    await loadRequestLogs(requestLogCursorStack[requestLogCursorStack.length - 1] ?? null, nextStack);
  }

  function requestLogQueryParams(cursor: RequestLogCursor | null) {
    const params = new URLSearchParams({
      limit: String(requestLogPageSize),
      date: requestLogDate,
    });

    if (requestLogNonSuccessOnly) {
      params.set("non_success_only", "true");
    }
    if (cursor) {
      params.set("before_started_at", cursor.before_started_at);
      params.set("before_request_id", cursor.before_request_id);
    }

    return params;
  }

  async function refreshActiveTab() {
    if (activePage === "tenants") {
      setTenantRefreshSignal((value) => value + 1);
      return;
    }
    if (activePage === "usage") {
      await loadUsage();
      return;
    }

    if (activePage === "users") {
      await loadUsers();
      return;
    }

    if (activePage === "plugins") {
      await Promise.all([loadPlugins(), loadPluginOptions()]);
      return;
    }

    if (activePage === "apiKeys") {
      await Promise.all([
        loadApiKeys(),
        loadPluginOptions(),
        currentUser?.role === "tenant_owner" ? loadProviderGroups() : loadProviderGroupOptions(),
      ]);
      return;
    }

    if (activePage === "requestLogs") {
      await loadRequestLogs(null, []);
      return;
    }

    if (currentUser?.role === "tenant_owner") {
      await Promise.all([loadAccounts(), loadProviderGroups()]);
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

  function navigateDashboard(path: string) {
    const normalized = normalizeDashboardPath(path, visibleRoutes);
    if (normalized === currentPath) {
      return;
    }
    window.history.pushState(null, "", normalized);
    setCurrentPath(normalized);
  }

  function openAccountImportDialog(mode: AccountImportMode = "oauth") {
    setAccountImportMode(activeAccountProvider === "claude" ? "oauth" : mode);
    setAuthorization(null);
    setCallbackUrl("");
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
    const requestSequence = ++unassignedProviderResourcesRequestSequenceRef.current;
    try {
      const params = new URLSearchParams({ provider });
      const resources = await requestJson<UnassignedProviderResource[]>(
        `${providerGroupsPath}/unassigned-resources?${params.toString()}`,
        undefined,
        token,
      );
      if (
        isActiveAuthToken(token) &&
        requestSequence === unassignedProviderResourcesRequestSequenceRef.current
      ) {
        setUnassignedProviderResources(resources);
      }
    } catch (error) {
      if (
        isActiveAuthToken(token) &&
        requestSequence === unassignedProviderResourcesRequestSequenceRef.current
      ) {
        showErrorToast("未分组资源加载失败", error);
      }
    } finally {
      if (
        isActiveAuthToken(token) &&
        requestSequence === unassignedProviderResourcesRequestSequenceRef.current
      ) {
        setUnassignedProviderResourcesLoading(false);
      }
    }
  }

  function closeProviderGroupCreateDialog() {
    if (!providerGroupSavingId) {
      unassignedProviderResourcesRequestSequenceRef.current += 1;
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
    setOfficialApiKey("");
    setOfficialBaseUrl(defaultUpstreamApiKeyBaseUrl(provider));
    setUpstreamApiKeyDialogProvider(provider);
  }

  function closeUpstreamApiKeyDialog() {
    if (saving) {
      return;
    }
    setUpstreamApiKeyDialogProvider(null);
    setOfficialApiKey("");
    setOfficialBaseUrl("https://api.openai.com/v1");
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
    setRequestOverrideTarget(target);
    setRequestOverrideHeaderRows(overrideEntriesFromObject(target.item.override.header));
    setRequestOverrideBodyRows(overrideEntriesFromObject(target.item.override.body));
  }

  function closeRequestOverrideDialog() {
    if (requestOverrideSaving) {
      return;
    }
    setRequestOverrideTarget(null);
    setRequestOverrideHeaderRows([]);
    setRequestOverrideBodyRows([]);
  }

  function openApiKeyCreateDialog() {
    setApiKeyName("");
    setApiKeyAllowedModels([]);
    setSelectedPluginReleaseId("");
    setSelectedProviderGroupId(providerGroupOptions[0]?.id ?? "");
    setApiKeyCreateOpen(true);
  }

  function closeApiKeyCreateDialog() {
    if (apiKeySaving) {
      return;
    }
    setApiKeyCreateOpen(false);
    setApiKeyName("");
    setApiKeyAllowedModels([]);
    setSelectedPluginReleaseId("");
    setSelectedProviderGroupId("");
  }

  function closeProviderGroupModelsDialog() {
    if (providerGroupModelsTarget?.id === providerGroupSavingId) {
      return;
    }
    setProviderGroupModelsTarget(null);
  }

  function closeApiKeyModelsDialog() {
    if (apiKeyModelsTarget?.id === apiKeyUpdatingId) {
      return;
    }
    setApiKeyModelsTarget(null);
  }

  function openApiKeyPluginDialog(apiKey: ApiKey) {
    setApiKeyPluginTarget(apiKey);
    setApiKeyPluginReleaseId(apiKey.plugin?.id ?? "");
  }

  function closeApiKeyPluginDialog() {
    if (apiKeyPluginSaving) {
      return;
    }
    setApiKeyPluginTarget(null);
    setApiKeyPluginReleaseId("");
  }

  function closePluginCreateDialog() {
    if (pluginSavingId === "create") {
      return;
    }
    setPluginCreateOpen(false);
  }

  function closePluginReleaseDialog() {
    if (pluginSavingId !== null) {
      return;
    }
    setPluginReleaseTarget(null);
  }

  function selectApiKeyProviderGroup(groupId: string) {
    setSelectedProviderGroupId(groupId);
    // 不保留上一个分组的模型选择，确保前端状态始终来自当前分组的候选集合。
    setApiKeyAllowedModels([]);
    setSelectedPluginReleaseId("");
  }

  function openUserQuotaDialog(user: DashboardUser) {
    setUserQuotaDialogUser(user);
    setUserQuotaValue(user.quota.toString());
  }

  function closeUserQuotaDialog() {
    if (userQuotaDialogUser && userUpdatingId === userQuotaDialogUser.id) {
      return;
    }
    setUserQuotaDialogUser(null);
    setUserQuotaValue("");
  }

  function openUserConcurrencyDialog(user: DashboardUser) {
    setUserConcurrencyDialogUser(user);
    setUserConcurrencyValue(user.max_concurrency?.toString() ?? "");
  }

  function closeUserConcurrencyDialog() {
    if (userConcurrencyDialogUser && userUpdatingId === userConcurrencyDialogUser.id) {
      return;
    }
    setUserConcurrencyDialogUser(null);
    setUserConcurrencyValue("");
  }

  function closeUserCreateDialog() {
    if (userCreating) {
      return;
    }
    setUserCreateOpen(false);
    setUserCreateUsername("");
    setUserCreateEmail("");
    setUserCreatePassword("");
  }

  function closeConfirmationDialog() {
    if (confirmationSubmitting) {
      return;
    }
    setConfirmationRequest(null);
  }

  async function confirmRequestedAction() {
    const request = confirmationRequest;
    if (!request || confirmationSubmitting) {
      return;
    }

    setConfirmationSubmitting(true);
    try {
      await request.onConfirm();
    } finally {
      // 无论请求成功或失败都关闭确认框，避免数据库已提交但运行态同步失败时误重放删除请求。
      setConfirmationRequest(null);
      setConfirmationSubmitting(false);
    }
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
    if (accountIds.length === 0 && apiKeyIds.length === 0) {
      toast.error("Provider 分组创建失败", { description: "至少需要选择一个未分组资源。" });
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
      toast.success(`${providerLabel(provider)} 分组已创建`);
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
      toast.success("Provider 分组名称已更新");
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
      toast.success(group.enabled ? "Provider 分组已停用" : "Provider 分组已恢复");
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
      description: `将删除分组“${group.name}”。受影响资源：${group.counts.account_count} 个 OAuth 账号和 ${group.counts.upstream_api_key_count} 个上游官方 Key 将解除分组、退出调度但保留凭证；${group.counts.gateway_api_key_count} 个调用方网关 Key 将永久删除。历史请求日志不受影响。`,
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
      await requestJson<DeleteProviderGroupResponse>(`${providerGroupsPath}/${group.id}`, {
        method: "DELETE",
      }, token);
      if (!isActiveAuthToken(token)) return;
      await Promise.all([loadProviderGroups(), loadAccounts(), loadApiKeys(apiKeysPage.offset)]);
      toast.success("Provider 分组已删除");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("Provider 分组删除失败", error);
        if (isProviderStateSyncError(error)) {
          await Promise.all([loadProviderGroups(), loadAccounts(), loadApiKeys(apiKeysPage.offset)]);
        }
      }
    } finally {
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
      await Promise.all([loadProviderGroups(), loadApiKeys(apiKeysPage.offset)]);
      toast.success("Provider 分组模型白名单已更新");
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
      toast.success("授权链接已生成", {
        description: isClaude
          ? "授权后请复制页面显示的 authorization code。"
          : "授权后请复制浏览器地址栏中的 callback URL。",
      });
    } catch (error) {
      showErrorToast("授权链接生成失败", error);
    } finally {
      setOauthLoading(false);
    }
  }

  async function submitCallback(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
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
      setCallbackUrl("");
      setAuthorization(null);
      setAccountImportOpen(false);
      toast.success(`${isClaude ? "Claude" : "GPT"} OAuth 账号已导入`);
      await Promise.all([
        loadAccounts(isClaude ? { claude: 0 } : { gpt: 0 }),
        loadProviderGroups(),
      ]);
    } catch (error) {
      showErrorToast("OAuth 账号导入失败", error);
      if (isProviderStateSyncError(error)) {
        // 数据库已经提交且 OAuth state 已消费，必须清空一次性输入并关闭弹窗，防止重放。
        setCallbackUrl("");
        setAuthorization(null);
        setAccountImportOpen(false);
        await Promise.all([
          loadAccounts(isClaude ? { claude: 0 } : { gpt: 0 }),
          loadProviderGroups(),
        ]);
      }
    } finally {
      setSaving(false);
    }
  }

  async function submitManualAccount(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
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
      setRefreshToken("");
      setClientId(defaultGptClientId);
      setChatgptAccountId("");
      setAccountImportOpen(false);
      toast.success("账号已保存");
      await Promise.all([loadAccounts({ gpt: 0 }), loadProviderGroups()]);
    } catch (error) {
      showErrorToast("账号保存失败", error);
      if (isProviderStateSyncError(error)) {
        setRefreshToken("");
        setClientId(defaultGptClientId);
        setChatgptAccountId("");
        setAccountImportOpen(false);
        await Promise.all([loadAccounts({ gpt: 0 }), loadProviderGroups()]);
      }
    } finally {
      setSaving(false);
    }
  }

  async function submitUpstreamApiKey(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
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
      setOfficialApiKey("");
      setOfficialBaseUrl(defaultUpstreamApiKeyBaseUrl(provider));
      toast.success(`${providerLabel(provider)} 官方 Key 已保存`);
    } catch (error) {
      showErrorToast("官方 Key 保存失败", error);
      if (isProviderStateSyncError(error)) {
        // 创建已经落库，销毁凭证输入并重新读取列表，不能让用户在原表单上再次提交。
        setUpstreamApiKeyDialogProvider(null);
        setOfficialApiKey("");
        setOfficialBaseUrl(defaultUpstreamApiKeyBaseUrl(provider));
        await Promise.all([
          loadAccounts(
            provider === "claude" ? { claudeUpstreamKeys: 0 } : { gptUpstreamKeys: 0 },
          ),
          loadProviderGroups(),
        ]);
      }
    } finally {
      setSaving(false);
    }
  }

  async function submitApiKey(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
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
      (plugin) => plugin.id === selectedPluginReleaseId,
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
      selectedPluginReleaseId &&
      (!selectedPlugin ||
        !selectedPlugin.suite_enabled ||
        selectedPlugin.provider !== selectedGroup.provider)
    ) {
      toast.error("API Key 创建失败", {
        description: "插件套件必须是当前 Provider 已启用的发布版本。",
      });
      return;
    }

    setApiKeySaving(true);
    try {
      await requestJson<ApiKey>(apiKeysPath, {
        method: "POST",
        body: JSON.stringify({
          name,
          group_id: selectedProviderGroupId,
          allowed_models: allowedModels,
          plugin_release_id: selectedPluginReleaseId || null,
        }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeyCreateOpen(false);
      setApiKeyName("");
      setApiKeyAllowedModels([]);
      setSelectedPluginReleaseId("");
      setSelectedProviderGroupId("");
      await loadApiKeys(0);
      if (currentUser?.role === "tenant_owner") {
        await loadProviderGroups();
      }
      toast.success("API Key 已创建");
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

  async function submitApiKeyPlugin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = authToken;
    const target = apiKeyPluginTarget;
    if (!token || !target) {
      return;
    }

    const selectedPlugin = pluginOptions.find(
      (plugin) => plugin.id === apiKeyPluginReleaseId,
    );
    if (
      apiKeyPluginReleaseId &&
      (!selectedPlugin ||
        !selectedPlugin.suite_enabled ||
        selectedPlugin.provider !== target.group.provider)
    ) {
      toast.error("插件绑定更新失败", {
        description: "请选择与 API Key Provider 一致的启用插件版本。",
      });
      return;
    }

    setApiKeyPluginSaving(true);
    try {
      const updated = await requestJson<ApiKey>(`${apiKeysPath}/${target.id}/plugin`, {
        method: "PUT",
        body: JSON.stringify({ plugin_release_id: apiKeyPluginReleaseId || null }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      setApiKeyPluginTarget(null);
      setApiKeyPluginReleaseId("");
      toast.success(updated.plugin ? "插件绑定已更新" : "插件绑定已解除");
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
      const updated = await requestJson<ApiKey>(`${apiKeysPath}/${apiKey.id}/models`, {
        method: "PUT",
        body: JSON.stringify({ allowed_models: allowedModels }),
      }, token);
      if (!isActiveAuthToken(token)) return false;
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      toast.success("API Key 模型白名单已更新");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("API Key 模型更新失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setApiKeyUpdatingId(null);
    }
  }

  async function createPlugin(input: CreatePluginInput): Promise<boolean> {
    const token = authToken;
    if (!token) return false;
    if (utf8ByteLength(input.name.trim()) > 128 || utf8ByteLength(input.description.trim()) > 1024) {
      toast.error("插件发布失败", {
        description: "名称最多 128 字节，描述最多 1024 字节。",
      });
      return false;
    }
    const artifactFiles = selectedPluginArtifactFiles(input);
    if (artifactFiles.length === 0) {
      toast.error("插件发布失败", { description: "请至少上传一个 WASM Component。" });
      return false;
    }
    if (artifactFiles.some(([, file]) => file.size === 0 || file.size > 8 * 1024 * 1024)) {
      toast.error("插件发布失败", {
        description: "每个 WASM 文件大小必须在 1 B 到 8 MiB 之间。",
      });
      return false;
    }

    const formData = new FormData();
    formData.set("name", input.name.trim());
    formData.set("description", input.description.trim());
    formData.set("provider", input.provider);
    for (const [field, file] of artifactFiles) formData.set(field, file);
    setPluginSavingId("create");
    try {
      await requestFormData<PluginReleaseSummary>(pluginsPath, formData, { method: "POST" }, token);
      if (!isActiveAuthToken(token)) return false;
      await Promise.all([loadPlugins(), loadPluginOptions()]);
      toast.success("插件已添加并发布");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件发布失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function publishPluginRelease(
    suiteId: string,
    files: PluginArtifactFiles,
  ): Promise<boolean> {
    const token = authToken;
    if (!token) return false;
    const artifactFiles = selectedPluginArtifactFiles(files);
    if (artifactFiles.length === 0) {
      toast.error("插件版本发布失败", { description: "请至少上传一个 WASM Component。" });
      return false;
    }
    if (artifactFiles.some(([, file]) => file.size === 0 || file.size > 8 * 1024 * 1024)) {
      toast.error("插件版本发布失败", {
        description: "每个 WASM 文件大小必须在 1 B 到 8 MiB 之间。",
      });
      return false;
    }
    const formData = new FormData();
    for (const [field, file] of artifactFiles) formData.set(field, file);
    setPluginSavingId(suiteId);
    try {
      await requestFormData<PluginReleaseSummary>(
        `${pluginsPath}/${suiteId}/releases`,
        formData,
        { method: "POST" },
        token,
      );
      if (!isActiveAuthToken(token)) return false;
      await Promise.all([loadPlugins(), loadPluginOptions()]);
      toast.success("插件新版本已发布");
      return true;
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件版本发布失败", error);
      return false;
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function togglePluginEnabled(plugin: PluginReleaseSummary) {
    const token = authToken;
    if (!token) return;
    setPluginSavingId(plugin.suite_id);
    try {
      const updated = await requestJson<PluginReleaseSummary[]>(
        `${pluginsPath}/${plugin.suite_id}/enabled`,
        { method: "PUT", body: JSON.stringify({ enabled: !plugin.suite_enabled }) },
        token,
      );
      if (!isActiveAuthToken(token)) return;
      setPlugins(updated);
      await loadPluginOptions();
      toast.success(plugin.suite_enabled ? "插件已停用" : "插件已启用");
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件状态更新失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  function requestDeletePlugin(plugin: PluginReleaseSummary) {
    setConfirmationRequest({
      title: "删除插件套件",
      description: `将永久删除插件“${plugin.suite_name}”的全部历史版本和 WASM Artifact。所有关联的网关 API Key都会保留，但会解除插件绑定，后续请求回落到 ${providerLabel(plugin.provider)} Provider 原生流程；历史请求日志不受影响。`,
      confirmLabel: "删除插件",
      pendingLabel: "正在删除",
      onConfirm: () => deletePlugin(plugin),
    });
  }

  async function deletePlugin(plugin: PluginReleaseSummary) {
    const token = authToken;
    if (!token || currentUser?.role !== "tenant_owner") {
      return;
    }

    setPluginSavingId(plugin.suite_id);
    try {
      const deleted = await requestJson<DeletePluginResponse>(
        `${pluginsPath}/${plugin.suite_id}`,
        { method: "DELETE" },
        token,
      );
      if (!isActiveAuthToken(token)) return;
      await Promise.all([
        loadPlugins(),
        loadPluginOptions(),
        loadApiKeys(apiKeysPage.offset),
      ]);
      toast.success("插件已删除", {
        description: `已删除 ${deleted.deleted_release_count} 个版本和 ${deleted.deleted_artifact_count} 个 Artifact，${deleted.unbound_gateway_api_key_count} 个网关 API Key已解除插件绑定。`,
      });
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("插件删除失败", error);
    } finally {
      if (isActiveAuthToken(token)) setPluginSavingId(null);
    }
  }

  async function toggleApiKeyEnabled(apiKey: ApiKey) {
    const token = authToken;
    if (!token) {
      return;
    }

    setApiKeyUpdatingId(apiKey.id);
    try {
      const updated = await requestJson<ApiKey>(`${apiKeysPath}/${apiKey.id}/enabled`, {
        method: "POST",
        body: JSON.stringify({ enabled: !apiKey.enabled }),
      }, token);
      if (!isActiveAuthToken(token)) {
        return;
      }
      setApiKeys((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      if (currentUser?.role === "tenant_owner") {
        await loadProviderGroups();
      }
      toast.success(apiKey.enabled ? "API Key 已禁用" : "API Key 已启用");
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
      description: `将永久删除网关 API Key“${apiKey.name}”，后续请求将立即无法再使用该凭证。所属 Provider 分组“${apiKey.group.name}”、上游资源和历史请求日志不受影响。`,
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
      const deleted = await requestJson<DeleteApiKeyResponse>(`${apiKeysPath}/${apiKey.id}`, {
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
        currentUser?.role === "tenant_owner" ? loadProviderGroups() : Promise.resolve(),
      ]);
      toast.success("API Key 已删除");
    } catch (error) {
      if (isActiveAuthToken(token)) showErrorToast("API Key 删除失败", error);
    } finally {
      if (isActiveAuthToken(token)) setApiKeyUpdatingId(null);
    }
  }

  function addRequestOverrideRow(section: "header" | "body") {
    const append = (rows: OverrideEntry[]) => [...rows, createOverrideEntry("", "")];
    if (section === "header") {
      setRequestOverrideHeaderRows(append);
    } else {
      setRequestOverrideBodyRows(append);
    }
  }

  function updateRequestOverrideRow(
    section: "header" | "body",
    id: string,
    field: "key" | "value",
    value: string,
  ) {
    const update = (rows: OverrideEntry[]) =>
      rows.map((row) => (row.id === id ? { ...row, [field]: value } : row));
    if (section === "header") {
      setRequestOverrideHeaderRows(update);
    } else {
      setRequestOverrideBodyRows(update);
    }
  }

  function removeRequestOverrideRow(section: "header" | "body", id: string) {
    const remove = (rows: OverrideEntry[]) => rows.filter((row) => row.id !== id);
    if (section === "header") {
      setRequestOverrideHeaderRows(remove);
    } else {
      setRequestOverrideBodyRows(remove);
    }
  }

  async function submitRequestOverride(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = authToken;
    const target = requestOverrideTarget;
    if (!token || currentUser?.role !== "tenant_owner" || !target) {
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
      setRequestOverrideHeaderRows([]);
      setRequestOverrideBodyRows([]);
      toast.success("请求覆盖已保存");
    } catch (error) {
      if (isActiveAuthToken(token)) {
        showErrorToast("请求覆盖保存失败", error);
        if (isProviderStateSyncError(error)) {
          setRequestOverrideTarget(null);
          setRequestOverrideHeaderRows([]);
          setRequestOverrideBodyRows([]);
          await loadAccounts();
        }
      }
    } finally {
      if (isActiveAuthToken(token)) {
        setRequestOverrideSaving(false);
      }
    }
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
      toast.success("GPT 账号分组已更新");
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
      toast.success("Claude 账号分组已更新");
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
      toast.success(`${providerLabel(provider)} 官方 Key 分组已更新`);
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
      const updated = await requestJson<GptAccount>(`/dash/gpt-accounts/${account.id}/enabled`, {
        method: "PUT",
        body: JSON.stringify({ enabled }),
      }, authToken);
      setAccounts((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      toast.success("账号调度开关已更新");
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
      toast.success("Claude 账号调度开关已更新");
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
      toast.success("官方 Key 调度开关已更新");
    } catch (error) {
      showErrorToast("官方 Key 调度开关更新失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      setUpstreamApiKeyEnabledUpdatingId(null);
    }
  }

  /** 查询并写入账号额度；重置成功后的静默同步与手动“查询额度”共用同一状态更新路径。 */
  async function fetchAndStoreAccountQuota(account: GptAccount) {
    const quota = await requestJson<GptAccountQuotaResponse>(
      `${gptAccountsPath}/${account.id}/quota`,
      {
        method: "POST",
      },
      authToken,
    );
    setAccountQuotas((items) => ({ ...items, [account.id]: quota }));
    if (quota.quota_limit_removed) {
      // 后端已同时更新 PostgreSQL quota 状态与 Redis 调度投影，重新加载账号列表，避免
      // 页面继续展示查询前的 quota_limited 快照。
      await loadAccounts();
    }
    return quota;
  }

  async function refreshAccountQuota(account: GptAccount) {
    setQuotaRefreshingIds((items) => ({ ...items, [account.id]: true }));
    try {
      const quota = await fetchAndStoreAccountQuota(account);
      if (quota.quota_limit_removed) {
        toast.success("账号额度查询成功，额度限制已解除");
      } else {
        toast.success("账号额度查询成功");
      }
    } catch (error) {
      showErrorToast("账号额度查询失败", error);
    } finally {
      setQuotaRefreshingIds((items) => {
        const next = { ...items };
        delete next[account.id];
        return next;
      });
    }
  }

  async function fetchRateLimitResetCredits(account: GptAccount) {
    return requestJson<RateLimitResetCreditsResponse>(
      `${gptAccountsPath}/${account.id}/rate-limit-reset-credits`,
      { method: "GET" },
      authToken,
    );
  }

  /** 点击“重置”后立即查询上游列表，弹窗同时承载加载态、错误态和最终兑换列表。 */
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
    if (!rateLimitResetTarget || applyingResetCreditId || rateLimitResetLoading) {
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
          toast.success(
            result.windows_reset > 0
              ? `额度重置已应用，共重置 ${result.windows_reset} 个窗口`
              : "额度重置已应用",
          );
          break;
        case "already_redeemed":
          toast.success("该次额度重置此前已成功应用");
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
            ? fetchAndStoreAccountQuota(account)
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
      setAccountQuotas((items) => {
        const next = { ...items };
        delete next[deleted.id];
        return next;
      });
      await loadProviderGroups();
      toast.success("账号已删除");
    } catch (error) {
      showErrorToast("账号删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
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
      toast.success("Claude 账号已删除");
    } catch (error) {
      showErrorToast("Claude 账号删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
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
      toast.success("官方 Key 已删除");
    } catch (error) {
      showErrorToast("官方 Key 删除失败", error);
      if (isProviderStateSyncError(error)) {
        await loadAccounts();
      }
    } finally {
      setUpstreamApiKeyDeletingId(null);
    }
  }

  async function submitUserQuota(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!userQuotaDialogUser) {
      return;
    }

    const quota = Number(userQuotaValue.trim());
    if (!Number.isSafeInteger(quota) || quota < 0 || quota > maxUserQuota) {
      toast.error("用户额度更新失败", {
        description: `额度必须是 0 到 ${maxUserQuota} 之间的安全整数。`,
      });
      return;
    }

    setUserUpdatingId(userQuotaDialogUser.id);
    try {
      const updated = await requestJson<DashboardUser>(`${usersPath}/${userQuotaDialogUser.id}/quota`, {
        method: "PUT",
        body: JSON.stringify({ quota }),
      }, authToken);
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      if (currentUser?.id === updated.id) {
        setCurrentUser(updated);
      }
      setUserQuotaDialogUser(null);
      setUserQuotaValue("");
      toast.success("用户额度已更新");
    } catch (error) {
      showErrorToast("用户额度更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }

  async function submitUserConcurrency(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!userConcurrencyDialogUser) {
      return;
    }

    const normalizedValue = userConcurrencyValue.trim();
    const maxConcurrency = normalizedValue === "" ? null : Number(normalizedValue);
    if (
      maxConcurrency !== null &&
      (!Number.isSafeInteger(maxConcurrency) || maxConcurrency < 1 || maxConcurrency > maxUserConcurrency)
    ) {
      toast.error("用户并发上限更新失败", {
        description: `并发上限必须留空，或填写 1 到 ${maxUserConcurrency} 之间的整数。`,
      });
      return;
    }

    setUserUpdatingId(userConcurrencyDialogUser.id);
    try {
      const updated = await requestJson<DashboardUser>(
        `${usersPath}/${userConcurrencyDialogUser.id}/max-concurrency`,
        {
          method: "PUT",
          body: JSON.stringify({ max_concurrency: maxConcurrency }),
        },
        authToken,
      );
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      if (currentUser?.id === updated.id) {
        setCurrentUser(updated);
      }
      setUserConcurrencyDialogUser(null);
      setUserConcurrencyValue("");
      toast.success("用户并发上限已更新");
    } catch (error) {
      showErrorToast("用户并发上限更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }

  async function submitUserCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const username = userCreateUsername.trim().toLowerCase();
    const usernameCharacters = Array.from(username);
    if (
      usernameCharacters.length === 0 ||
      usernameCharacters.length > 32 ||
      utf8ByteLength(username) > 128 ||
      !/^[\p{L}\p{N}][\p{L}\p{N}_-]*$/u.test(username)
    ) {
      toast.error("用户添加失败", {
        description: "用户名最多 32 个字符，只能包含字母、数字、下划线和连字符。",
      });
      return;
    }
    if (Array.from(userCreatePassword).length < 8 || utf8ByteLength(userCreatePassword) > 72) {
      toast.error("用户添加失败", {
        description: "密码至少 8 个字符，且 UTF-8 编码长度不能超过 72 字节。",
      });
      return;
    }

    const email = userCreateEmail.trim();
    setUserCreating(true);
    try {
      await requestJson<DashboardUser>(usersPath, {
        method: "POST",
        // 邮箱留空时不发送该字段，确保默认地址规则由服务端作为唯一真源执行。
        body: JSON.stringify({
          username,
          ...(email ? { email } : {}),
          password: userCreatePassword,
        }),
      }, authToken);
      setUserCreateOpen(false);
      setUserCreateUsername("");
      setUserCreateEmail("");
      setUserCreatePassword("");
      await loadUsers(0);
      toast.success("用户已添加");
    } catch (error) {
      showErrorToast("用户添加失败", error);
    } finally {
      setUserCreating(false);
    }
  }

  async function updateUserStatus(user: DashboardUser) {
    setUserUpdatingId(user.id);
    try {
      const updated = await requestJson<DashboardUser>(`${usersPath}/${user.id}/status`, {
        method: "PUT",
        body: JSON.stringify({ enabled: !user.enabled }),
      }, authToken);
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      toast.success(updated.enabled ? "用户已启用" : "用户已禁用");
    } catch (error) {
      showErrorToast("用户状态更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }

  async function copyAuthorizationUrl() {
    if (!authorization) {
      return;
    }
    try {
      await navigator.clipboard.writeText(authorization.authorization_url);
      toast.success("授权链接已复制");
    } catch (error) {
      showErrorToast("授权链接复制失败", error);
    }
  }

  async function copyApiKey(apiKey: ApiKey) {
    try {
      await navigator.clipboard.writeText(apiKey.api_key);
      toast.success("API Key 已复制");
    } catch (error) {
      showErrorToast("API Key 复制失败", error);
    }
  }

  const resetOperationAccountId =
    rateLimitResetTarget && (rateLimitResetLoading || applyingResetCreditId)
      ? rateLimitResetTarget.id
      : null;

  if (authLoading || !currentUser) {
    return (
      <AuthScreen
        theme={theme}
        loading={authLoading}
        mode={authMode}
        submitting={authSubmitting}
        emailCodeSending={emailCodeSending}
        loginIdentifier={loginIdentifier}
        loginPassword={loginPassword}
        registerUsername={registerUsername}
        registerTenantCode={registerTenantCode}
        registerEmail={registerEmail}
        registerPassword={registerPassword}
        registerCode={registerCode}
        onModeChange={setAuthMode}
        onLoginIdentifierChange={setLoginIdentifier}
        onLoginPasswordChange={setLoginPassword}
        onRegisterUsernameChange={setRegisterUsername}
        onRegisterTenantCodeChange={setRegisterTenantCode}
        onRegisterEmailChange={setRegisterEmail}
        onRegisterPasswordChange={setRegisterPassword}
        onRegisterCodeChange={setRegisterCode}
        onLogin={submitLogin}
        onRegister={submitRegister}
        onSendEmailCode={sendRegisterEmailCode}
      />
    );
  }

  return (
    <DashboardShell
      activePage={activePage}
      activeRoute={activeRoute}
      routes={visibleRoutes}
      currentUser={currentUser}
      tenant={currentTenant}
      theme={theme}
      refreshing={activePageLoading(
        activePage,
        providerGroupsVisible ? providerGroupsLoading : loading,
        usersLoading,
        pluginsLoading,
        apiKeysLoading,
        requestLogsLoading,
        usageLoading,
      )}
      overlays={
        <AnimatePresence>
          {accountImportOpen && activePage === "accounts" && (
            <AccountImportDialog
              key="account-import"
              provider={activeAccountProvider}
              providerLabel={activeAccountProviderMeta.label}
              mode={accountImportMode}
              authorization={authorization}
              callbackUrl={callbackUrl}
              refreshToken={refreshToken}
              clientId={clientId}
              chatgptAccountId={chatgptAccountId}
              saving={saving}
              oauthLoading={oauthLoading}
              onClose={closeAccountImportDialog}
              onModeChange={setAccountImportMode}
              onCreateAuthorization={createAuthorization}
              onCopyAuthorizationUrl={copyAuthorizationUrl}
              onCallbackUrlChange={setCallbackUrl}
              onRefreshTokenChange={setRefreshToken}
              onClientIdChange={setClientId}
              onChatgptAccountIdChange={setChatgptAccountId}
              onSubmitCallback={submitCallback}
              onSubmitManual={submitManualAccount}
            />
          )}
          {rateLimitResetTarget && activePage === "accounts" && (
            <RateLimitResetDialog
              key={`rate-limit-reset-${rateLimitResetTarget.id}`}
              account={rateLimitResetTarget}
              response={rateLimitResetResponse}
              loading={rateLimitResetLoading}
              error={rateLimitResetError}
              applyingCreditId={applyingResetCreditId}
              onApply={applyRateLimitResetCredit}
              onClose={closeRateLimitResetDialog}
            />
          )}
          {providerGroupCreateProvider && activePage === "accounts" && (
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
          {providerGroupModelsTarget && activePage === "accounts" && (
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
          {upstreamApiKeyDialogProvider && activePage === "accounts" && (
            <ProviderUpstreamApiKeyDialog
              key="upstream-api-key"
              providerLabel={providerLabel(upstreamApiKeyDialogProvider)}
              apiKey={officialApiKey}
              baseUrl={officialBaseUrl}
              baseUrlPlaceholder={defaultUpstreamApiKeyBaseUrl(upstreamApiKeyDialogProvider)}
              saving={saving}
              onApiKeyChange={setOfficialApiKey}
              onBaseUrlChange={setOfficialBaseUrl}
              onSubmit={submitUpstreamApiKey}
              onClose={closeUpstreamApiKeyDialog}
            />
          )}
          {requestOverrideTarget && activePage === "accounts" && (
            <RequestOverrideDialog
              key="request-override"
              target={requestOverrideTarget}
              headerRows={requestOverrideHeaderRows}
              bodyRows={requestOverrideBodyRows}
              saving={requestOverrideSaving}
              onAdd={addRequestOverrideRow}
              onChange={updateRequestOverrideRow}
              onRemove={removeRequestOverrideRow}
              onSubmit={submitRequestOverride}
              onClose={closeRequestOverrideDialog}
            />
          )}
          {apiKeyCreateOpen && activePage === "apiKeys" && (
            <ApiKeyCreateDialog
              key="api-key-create"
              name={apiKeyName}
              selectedModels={apiKeyAllowedModels}
              saving={apiKeySaving}
              groups={providerGroupOptions}
              groupId={selectedProviderGroupId}
              plugins={pluginOptions}
              pluginReleaseId={selectedPluginReleaseId}
              onNameChange={setApiKeyName}
              onModelsChange={setApiKeyAllowedModels}
              onGroupChange={selectApiKeyProviderGroup}
              onPluginChange={setSelectedPluginReleaseId}
              onSubmit={submitApiKey}
              onClose={closeApiKeyCreateDialog}
            />
          )}
          {apiKeyModelsTarget && activePage === "apiKeys" && (
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
          {apiKeyPluginTarget && activePage === "apiKeys" && (
            <ApiKeyPluginDialog
              key={`api-key-plugin-${apiKeyPluginTarget.id}`}
              apiKey={apiKeyPluginTarget}
              plugins={pluginOptions}
              pluginReleaseId={apiKeyPluginReleaseId}
              saving={apiKeyPluginSaving}
              onPluginChange={setApiKeyPluginReleaseId}
              onSubmit={submitApiKeyPlugin}
              onClose={closeApiKeyPluginDialog}
            />
          )}
          {pluginCreateOpen && activePage === "plugins" && (
            <PluginCreateDialog
              key="plugin-create"
              saving={pluginSavingId === "create"}
              onCreate={createPlugin}
              onClose={closePluginCreateDialog}
            />
          )}
          {pluginReleaseTarget && activePage === "plugins" && (
            <PluginReleaseDialog
              key={`plugin-release-${pluginReleaseTarget.suite_id}`}
              plugin={pluginReleaseTarget}
              saving={pluginSavingId === pluginReleaseTarget.suite_id}
              onPublish={publishPluginRelease}
              onClose={closePluginReleaseDialog}
            />
          )}
          {userQuotaDialogUser && activePage === "users" && (
            <UserQuotaDialog
              key="user-quota"
              user={userQuotaDialogUser}
              value={userQuotaValue}
              saving={userUpdatingId === userQuotaDialogUser.id}
              onValueChange={setUserQuotaValue}
              onSubmit={submitUserQuota}
              onClose={closeUserQuotaDialog}
            />
          )}
          {userConcurrencyDialogUser && activePage === "users" && (
            <UserConcurrencyDialog
              key="user-concurrency"
              user={userConcurrencyDialogUser}
              value={userConcurrencyValue}
              maxValue={maxUserConcurrency}
              saving={userUpdatingId === userConcurrencyDialogUser.id}
              onValueChange={setUserConcurrencyValue}
              onSubmit={submitUserConcurrency}
              onClose={closeUserConcurrencyDialog}
            />
          )}
          {userCreateOpen && activePage === "users" && (
            <UserCreateDialog
              key="user-create"
              username={userCreateUsername}
              email={userCreateEmail}
              password={userCreatePassword}
              saving={userCreating}
              onUsernameChange={setUserCreateUsername}
              onEmailChange={setUserCreateEmail}
              onPasswordChange={setUserCreatePassword}
              onSubmit={submitUserCreate}
              onClose={closeUserCreateDialog}
            />
          )}
          {confirmationRequest && (
            <ConfirmDialog
              key="confirmation"
              title={confirmationRequest.title}
              description={confirmationRequest.description}
              confirmLabel={confirmationRequest.confirmLabel}
              pendingLabel={confirmationRequest.pendingLabel}
              pending={confirmationSubmitting}
              onConfirm={confirmRequestedAction}
              onClose={closeConfirmationDialog}
            />
          )}
        </AnimatePresence>
      }
      onNavigate={navigateDashboard}
      onRefresh={refreshActiveTab}
      onLogout={logout}
      onToggleTheme={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
    >
      {activePage === "tenants" ? (
        <TenantsPage token={authToken ?? ""} refreshSignal={tenantRefreshSignal} />
      ) : activePage === "usage" ? (
        <Suspense
          fallback={
            <div className="flex min-h-64 items-center justify-center rounded-xl border border-slate-200 bg-white p-8 text-sm text-slate-500 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-400">
              正在加载图表组件
            </div>
          }
        >
          <UsagePage
            theme={theme}
            usage={usage}
            loading={usageLoading}
          />
        </Suspense>
      ) : activePage === "accounts" ? (
        <AccountsPage
          accounts={accounts}
          claudeAccounts={claudeAccounts}
          gptUpstreamApiKeys={gptUpstreamApiKeys}
          claudeUpstreamApiKeys={claudeUpstreamApiKeys}
          accountQuotas={accountQuotas}
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
          quotaRefreshingIds={quotaRefreshingIds}
          resetOperationAccountId={resetOperationAccountId}
          providerGroups={providerGroups}
          resourceGroupUpdatingId={resourceGroupUpdatingId}
          pageOffset={activeCredentialPage.offset}
          pageSize={dashboardListPageSize}
          nextPageOffset={activeCredentialPage.nextOffset}
          onProviderChange={(provider) => {
            setActiveAccountProvider(provider);
            setActiveCredentialTab("accounts");
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
          onRefreshAccountQuota={refreshAccountQuota}
          onOpenRateLimitReset={openRateLimitResetDialog}
          onDeleteGptAccount={requestDeleteAccount}
          onDeleteClaudeAccount={requestDeleteClaudeAccount}
          onDeleteUpstreamApiKey={requestDeleteUpstreamApiKey}
          onOpenRequestOverride={openRequestOverrideDialog}
          onPageChange={loadActiveCredentialPage}
        />
      ) : activePage === "plugins" ? (
        <PluginsPage
          plugins={plugins}
          loading={pluginsLoading}
          savingId={pluginSavingId}
          onAdd={() => setPluginCreateOpen(true)}
          onOpenPublish={setPluginReleaseTarget}
          onToggleEnabled={togglePluginEnabled}
          onDelete={requestDeletePlugin}
        />
      ) : activePage === "users" ? (
        <UsersPage
          users={users}
          loading={usersLoading}
          updatingId={userUpdatingId}
          currentUserId={currentUser.id}
          offset={usersPage.offset}
          pageSize={dashboardListPageSize}
          nextOffset={usersPage.nextOffset}
          onAdd={() => setUserCreateOpen(true)}
          onOpenQuota={openUserQuotaDialog}
          onOpenConcurrency={openUserConcurrencyDialog}
          onToggleStatus={updateUserStatus}
          onPageChange={loadUsers}
        />
      ) : activePage === "apiKeys" ? (
        <ApiKeysPage
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
        />
      ) : (
        <RequestLogsPage
          logs={requestLogs}
          showUsername={currentUser.role !== "tenant_user"}
          loading={requestLogsLoading}
          date={requestLogDate}
          minDate={shiftDateInputValue(
            todayInputValue(serviceTimezone),
            -(requestLogRetentionDays - 1),
          )}
          maxDate={todayInputValue(serviceTimezone)}
          timezone={serviceTimezone}
          nonSuccessOnly={requestLogNonSuccessOnly}
          nextCursor={requestLogNextCursor}
          cursorStack={requestLogCursorStack}
          onDateChange={setRequestLogDate}
          onNonSuccessOnlyChange={setRequestLogNonSuccessOnly}
          onPreviousPage={loadPreviousRequestLogPage}
          onNextPage={loadNextRequestLogPage}
        />
      )}
    </DashboardShell>
  );
}

interface ListPageState {
  offset: number;
  nextOffset: number | null;
}

interface AccountPageOffsets {
  gpt: number;
  claude: number;
  gptUpstreamKeys: number;
  claudeUpstreamKeys: number;
}

function initialListPageState(): ListPageState {
  return { offset: 0, nextOffset: null };
}

function pageStateFrom(page: { offset: number; next_offset: number | null }): ListPageState {
  return { offset: page.offset, nextOffset: page.next_offset };
}

function listPagePath(path: string, offset: number) {
  const params = new URLSearchParams({
    limit: String(dashboardListPageSize),
    offset: String(offset),
  });
  return `${path}?${params.toString()}`;
}

function utf8ByteLength(value: string) {
  return new TextEncoder().encode(value).byteLength;
}

/** 将三插槽表单归一化为后端 multipart 字段；未选择的插槽不发送。 */
function selectedPluginArtifactFiles(files: PluginArtifactFiles): Array<[string, File]> {
  const selected: Array<[string, File]> = [];
  if (files.requestFile) selected.push(["request_file", files.requestFile]);
  if (files.bufferedResponseFile) {
    selected.push(["buffered_response_file", files.bufferedResponseFile]);
  }
  if (files.streamResponseFile) {
    selected.push(["stream_response_file", files.streamResponseFile]);
  }
  return selected;
}

function isAscii(value: string) {
  return Array.from(value).every((character) => character.codePointAt(0)! <= 0x7f);
}

function asUpstreamApiKeyProvider(
  provider: AccountProviderKey,
): UpstreamApiKeyProvider | null {
  return provider === "gpt" || provider === "claude" ? provider : null;
}

function upstreamApiKeyPath(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? claudeUpstreamApiKeysPath : gptUpstreamApiKeysPath;
}

function defaultUpstreamApiKeyBaseUrl(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? "https://api.anthropic.com" : "https://api.openai.com/v1";
}

function providerLabel(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? "Claude" : "GPT";
}
