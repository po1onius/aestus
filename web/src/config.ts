import type { AccountProviderKey, DashboardPage, DashboardRoute } from "./types";

export const statusOptions = [
  { value: "active", label: "启用" },
  { value: "disabled", label: "禁用" },
  { value: "valid", label: "可用" },
  { value: "unauthorized", label: "待刷新" },
  { value: "invalid", label: "凭证无效" },
  { value: "unavailable", label: "待探活" },
];

export const accountProviderTabs: Array<{
  key: AccountProviderKey;
  label: string;
  ready: boolean;
}> = [
    { key: "gpt", label: "GPT", ready: true },
    { key: "claude", label: "Claude", ready: true },
    { key: "grok", label: "Grok", ready: false },
  ];

// 控制台 API 与页面路由分别集中定义，调用方只拼接资源 ID 和操作后缀。
export const consoleApiPath = "/api/console";
export const authPath = `${consoleApiPath}/auth`;
export const gptAccountsPath = `${consoleApiPath}/providers/gpt/accounts`;
export const claudeAccountsPath = `${consoleApiPath}/providers/claude/accounts`;
export const claudeUpstreamApiKeysPath = `${consoleApiPath}/providers/claude/upstream-api-keys`;
export const gptUpstreamApiKeysPath = `${consoleApiPath}/providers/gpt/upstream-api-keys`;
export const gatewayApiKeysPath = `${consoleApiPath}/gateway-api-keys`;
export const pluginsPath = `${consoleApiPath}/plugins`;
export const pluginSuitesPath = `${consoleApiPath}/plugin-suites`;
export const providerGroupsPath = `${consoleApiPath}/provider-groups`;
export const requestLogsPath = `${consoleApiPath}/request-logs`;
export const policyLogsPath = `${consoleApiPath}/policy-logs`;
export const usagePath = `${consoleApiPath}/usage`;
export const usersPath = `${consoleApiPath}/users`;
export const tenantsPath = `${consoleApiPath}/tenants`;
export const authTokenStorageKey = "aestus_dashboard_token";
export const themeStorageKey = "aestus_dashboard_theme";
export const requestLogPageSize = 100;
export const dashboardListPageSize = 100;
export const maxUserQuota = Number.MAX_SAFE_INTEGER;
export const maxUserConcurrency = 10_000;
export const defaultGptClientId = "app_EMoamEEZ73f0CkXaXp7hrann";

export const consolePagePaths = {
  tenants: "/console/tenants",
  providers: "/console/providers",
  plugins: "/console/plugins",
  users: "/console/users",
  usage: "/console/usage",
  gatewayApiKeys: "/console/gateway-api-keys",
  requestLogs: "/console/request-logs",
} satisfies Record<DashboardPage, string>;

export const dashboardRoutes: DashboardRoute[] = [
  {
    page: "tenants",
    path: consolePagePaths.tenants,
    label: "租户",
    platformOnly: true,
  },
  {
    page: "providers",
    path: consolePagePaths.providers,
    label: "Provider",
    ownerOnly: true,
  },
  {
    page: "plugins",
    path: consolePagePaths.plugins,
    label: "插件",
    ownerOnly: true,
  },
  {
    page: "users",
    path: consolePagePaths.users,
    label: "用户",
    ownerOnly: true,
  },
  {
    page: "usage",
    path: consolePagePaths.usage,
    label: "用量概览",
  },
  {
    page: "gatewayApiKeys",
    path: consolePagePaths.gatewayApiKeys,
    label: "API Key",
    tenantOnly: true,
  },
  {
    page: "requestLogs",
    path: consolePagePaths.requestLogs,
    label: "请求日志",
  },
];
