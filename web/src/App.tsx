import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { DashboardShell } from "./components/DashboardShell";
import { dashboardRoutes, themeStorageKey } from "./config";
import { AuthController } from "./features/auth/AuthController";
import { useSession } from "./features/auth/useSession";
import { DashboardContext } from "./features/dashboard/context";
import { useCurrentGroupAccess } from "./features/group-access/access";
import { normalizeDashboardPath, pageFromPath, routesForUser } from "./lib/routing";
import type { DashboardTheme } from "./types";

const ProvidersScreen = lazy(() => import("./features/accounts/ProvidersScreen").then(m => ({ default: m.ProvidersScreen })));
const GatewayApiKeysScreen = lazy(() => import("./features/api-keys/GatewayApiKeysScreen").then(m => ({ default: m.GatewayApiKeysScreen })));
const PluginsScreen = lazy(() => import("./features/plugins/PluginsScreen").then(m => ({ default: m.PluginsScreen })));
const UsersScreen = lazy(() => import("./features/users/UsersScreen").then(m => ({ default: m.UsersScreen })));
const RequestLogsScreen = lazy(() => import("./features/request-logs/RequestLogsScreen").then(m => ({ default: m.RequestLogsScreen })));
const UsageScreen = lazy(() => import("./features/usage/UsageScreen").then(m => ({ default: m.UsageScreen })));
const TenantsPage = lazy(() => import("./pages/TenantsPage").then(m => ({ default: m.TenantsPage })));
const AuditLogsPage = lazy(() => import("./pages/AuditLogsPage").then(m => ({ default: m.AuditLogsPage })));

export function App() {
  const session = useSession();
  const [theme, setTheme] = useState<DashboardTheme>(() => localStorage.getItem(themeStorageKey) === "dark" ? "dark" : "light");
  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    document.documentElement.style.colorScheme = theme;
    localStorage.setItem(themeStorageKey, theme);
  }, [theme]);

  if (!session.session) {
    return <AuthController key={session.token ?? "anonymous"} theme={theme} loading={session.loading} applyAuth={session.applyAuth} />;
  }
  // token 变化时整体销毁控制台，各页面的表单、分页和在途请求不跨会话保留。
  return <Dashboard key={session.session.token} session={session} theme={theme} onToggleTheme={() => setTheme(value => value === "dark" ? "light" : "dark")} />;
}

function Dashboard({ session, theme, onToggleTheme }: {
  session: ReturnType<typeof useSession>;
  theme: DashboardTheme;
  onToggleTheme: () => void;
}) {
  const data = session.session!;
  const currentGroupAccess = useCurrentGroupAccess(data.user, data.token);
  const [currentPath, setCurrentPath] = useState(() => normalizeDashboardPath(window.location.pathname));
  const [refreshRevision, setRefreshRevision] = useState(0);
  const [loadingPage, setLoadingPage] = useState<string | null>(null);
  const routes = useMemo(() => routesForUser(data.user, currentGroupAccess.access.canViewProviderResources), [data.user.role, currentGroupAccess.access.canViewProviderResources]);
  const activePage = pageFromPath(currentPath, routes);
  const activeRoute = routes.find(route => route.page === activePage) ?? routes[0] ?? dashboardRoutes[0];

  useEffect(() => {
    if (!currentGroupAccess.ready) return;
    function updatePath() {
      const normalized = normalizeDashboardPath(window.location.pathname, routes);
      if (normalized !== window.location.pathname) window.history.replaceState(null, "", normalized);
      setCurrentPath(normalized);
    }
    updatePath();
    window.addEventListener("popstate", updatePath);
    return () => window.removeEventListener("popstate", updatePath);
  }, [routes, currentGroupAccess.ready]);

  const onLoadingChange = useCallback((loading: boolean) => {
    setLoadingPage(previous => loading ? activePage : previous === activePage ? null : previous);
  }, [activePage]);
  const context = useMemo(() => ({
    authToken: data.token, currentUser: data.user, setCurrentUser: session.setCurrentUser,
    serviceTimezone: data.service_timezone, requestLogRetentionDays: data.request_log_retention_days,
    theme, providerAccess: currentGroupAccess.access,
  }), [data, session.setCurrentUser, theme, currentGroupAccess.access]);
  const props = { refreshRevision, onLoadingChange };

  return <DashboardContext.Provider value={context}>
    <DashboardShell activePage={activePage} activeRoute={activeRoute} routes={routes}
      currentUser={data.user} tenant={data.tenant} theme={theme} refreshing={loadingPage === activePage || currentGroupAccess.loading}
      onNavigate={path => {
        const normalized = normalizeDashboardPath(path, routes);
        if (normalized !== currentPath) { window.history.pushState(null, "", normalized); setCurrentPath(normalized); }
      }}
      onRefresh={() => { setRefreshRevision(value => value + 1); if (data.user.role === "tenant_user") void currentGroupAccess.reload(); }}
      onLogout={session.logout} onToggleTheme={onToggleTheme}>
      {currentGroupAccess.ready ? <Suspense key={activePage} fallback={<p role="status">正在加载页面…</p>}>
        {activePage === "providers" ? <ProvidersScreen {...props} /> :
          activePage === "gatewayApiKeys" ? <GatewayApiKeysScreen {...props} /> :
            activePage === "plugins" ? <PluginsScreen {...props} /> :
              activePage === "users" ? <UsersScreen {...props} /> :
                activePage === "requestLogs" ? <RequestLogsScreen {...props} /> :
                  activePage === "usage" ? <UsageScreen {...props} /> :
                    activePage === "auditLogs" ? <AuditLogsPage token={data.token} timezone={data.service_timezone} role={data.user.role} refreshRevision={refreshRevision} onLoadingChange={onLoadingChange} /> :
                      <TenantsPage token={data.token} refreshSignal={refreshRevision} />}
      </Suspense> : <p role="status">正在加载分组权限…</p>}
    </DashboardShell>
  </DashboardContext.Provider>;
}
