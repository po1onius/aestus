import type { AccountProviderKey, DashboardRoute } from "./types";

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

export const gptAccountsPath = "/dash/gpt-accounts";
export const claudeAccountsPath = "/dash/claude-accounts";
export const claudeUpstreamApiKeysPath = "/dash/claude-upstream-api-keys";
export const gptUpstreamApiKeysPath = "/dash/gpt-upstream-api-keys";
export const apiKeysPath = "/dash/api-keys";
export const pluginsPath = "/dash/plugins";
export const providerGroupsPath = "/dash/provider-groups";
export const requestLogsPath = "/dash/request-logs";
export const usagePath = "/dash/usage";
export const usersPath = "/dash/users";
export const authTokenStorageKey = "aestus_dashboard_token";
export const themeStorageKey = "aestus_dashboard_theme";
export const requestLogPageSize = 100;
export const dashboardListPageSize = 100;
export const maxUserQuota = Number.MAX_SAFE_INTEGER;
export const defaultGptClientId = "app_EMoamEEZ73f0CkXaXp7hrann";

export const dashboardRoutes: DashboardRoute[] = [
  {
    page: "accounts",
    path: "/admin/accounts",
    label: "Provider",
    adminOnly: true,
  },
  {
    page: "plugins",
    path: "/admin/plugins",
    label: "插件",
    adminOnly: true,
  },
  {
    page: "users",
    path: "/admin/users",
    label: "用户",
    adminOnly: true,
  },
  {
    page: "usage",
    path: "/admin/usage",
    label: "用量概览",
    adminOnly: true,
  },
  {
    page: "usage",
    path: "/dashboard/usage",
    label: "用量概览",
    userOnly: true,
  },
  {
    page: "apiKeys",
    path: "/admin/api-keys",
    label: "API Key",
  },
  {
    page: "requestLogs",
    path: "/admin/request-logs",
    label: "请求日志",
  },
];
