import { consolePagePaths, dashboardRoutes } from "../config";
import type { DashboardPage, DashboardRoute, DashboardUser } from "../types";

export function routesForUser(
  user: DashboardUser | null,
  canViewProviderResources = false,
) {
  if (!user) {
    return dashboardRoutes.filter((route) => !route.platformOnly && !route.ownerOnly);
  }
  if (user.role === "platform_admin") {
    return dashboardRoutes.filter((route) => route.page === "plugins" || route.platformOnly || (!route.ownerOnly && !route.tenantOnly));
  }
  if (user.role === "tenant_owner") {
    return dashboardRoutes.filter((route) => !route.platformOnly);
  }
  return dashboardRoutes.filter(
    (route) =>
      !route.platformOnly &&
      (!route.ownerOnly || (route.page === "providers" && canViewProviderResources)),
  );
}

export function pageFromPath(pathname: string, routes: DashboardRoute[]): DashboardPage {
  const normalized = stripTrailingSlash(pathname);
  return routes.find((route) => route.path === normalized)?.page ?? routes[0]?.page ?? "gatewayApiKeys";
}

export function normalizeDashboardPath(
  pathname: string,
  routes: DashboardRoute[] = dashboardRoutes,
) {
  const normalized = stripTrailingSlash(pathname);
  return routes.some((route) => route.path === normalized)
    ? normalized
    : (routes[0]?.path ?? consolePagePaths.gatewayApiKeys);
}

function stripTrailingSlash(pathname: string) {
  return pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
}
