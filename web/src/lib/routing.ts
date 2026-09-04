import { dashboardRoutes } from "../config";
import type { DashboardPage, DashboardRoute, DashboardUser } from "../types";

export function routesForUser(
  user: DashboardUser | null,
  canViewProviderResources = false,
) {
  if (!user) {
    return dashboardRoutes.filter((route) => !route.platformOnly && !route.ownerOnly);
  }
  if (user.role === "platform_admin") {
    return dashboardRoutes.filter((route) => route.platformOnly || (!route.ownerOnly && !route.tenantOnly));
  }
  if (user.role === "tenant_owner") {
    return dashboardRoutes.filter((route) => !route.platformOnly);
  }
  return dashboardRoutes.filter(
    (route) =>
      !route.platformOnly &&
      (!route.ownerOnly || (route.page === "accounts" && canViewProviderResources)),
  );
}

export function pageFromPath(pathname: string, routes: DashboardRoute[]): DashboardPage {
  const normalized = stripTrailingSlash(pathname);
  return routes.find((route) => route.path === normalized)?.page ?? routes[0]?.page ?? "apiKeys";
}

export function normalizeDashboardPath(
  pathname: string,
  routes: DashboardRoute[] = dashboardRoutes,
) {
  const normalized = stripTrailingSlash(pathname);
  return routes.some((route) => route.path === normalized)
    ? normalized
    : (routes[0]?.path ?? "/admin/api-keys");
}

function stripTrailingSlash(pathname: string) {
  return pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
}

export function activePageLoading(
  activePage: DashboardPage,
  accountsLoading: boolean,
  usersLoading: boolean,
  pluginsLoading: boolean,
  apiKeysLoading: boolean,
  requestLogsLoading: boolean,
  usageLoading: boolean,
) {
  if (activePage === "usage") {
    return usageLoading;
  }
  if (activePage === "users") {
    return usersLoading;
  }
  if (activePage === "plugins") {
    return pluginsLoading;
  }
  if (activePage === "apiKeys") {
    return apiKeysLoading;
  }
  if (activePage === "requestLogs") {
    return requestLogsLoading;
  }
  return accountsLoading;
}
