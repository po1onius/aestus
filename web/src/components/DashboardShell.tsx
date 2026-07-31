import {
  ChartNoAxesCombined,
  KeyRound,
  Loader2,
  LogOut,
  Moon,
  PlugZap,
  RefreshCw,
  ScrollText,
  ServerCog,
  Sun,
  UsersRound,
} from "lucide-react";
import type { ReactNode } from "react";
import { Toaster } from "sonner";
import tokenGatewayLogo from "../assets/token-gateway-logo.svg";
import { cx, iconButton, spinnerClass } from "../lib/ui";
import type { DashboardPage, DashboardRoute, DashboardTheme, DashboardUser } from "../types";

interface DashboardShellProps {
  activePage: DashboardPage;
  activeRoute: DashboardRoute;
  routes: DashboardRoute[];
  currentUser: DashboardUser;
  theme: DashboardTheme;
  refreshing: boolean;
  overlays: ReactNode;
  children: ReactNode;
  onNavigate: (path: string) => void;
  onRefresh: () => void;
  onLogout: () => void;
  onToggleTheme: () => void;
}

// 侧栏条目高度 40px、间距 4px，对应 Tailwind 的 11 个 spacing 单位。
const sideNavIndicatorPositions = [
  "lg:translate-y-0",
  "lg:translate-y-11",
  "lg:translate-y-22",
  "lg:translate-y-33",
  "lg:translate-y-44",
  "lg:translate-y-55",
] as const;

/**
 * 管理面板的稳定壳层：统一侧栏、顶栏、Toast 和浮层挂载位置。
 * 领域页面作为 children 注入，因此新增 provider 或页面时无需改动壳层布局。
 */
export function DashboardShell({
  activePage,
  activeRoute,
  routes,
  currentUser,
  theme,
  refreshing,
  overlays,
  children,
  onNavigate,
  onRefresh,
  onLogout,
  onToggleTheme,
}: DashboardShellProps) {
  const panelLabel = "控制台";
  const activeRouteIndex = Math.max(0, routes.findIndex((route) => route.page === activePage));

  return (
    <>
      <Toaster position="top-right" theme={theme} richColors closeButton />
      <main
        className={cx(
          "min-h-screen bg-slate-50 text-slate-950 transition-colors dark:bg-slate-950 dark:text-slate-100 lg:grid lg:grid-cols-[15rem_minmax(0,1fr)]",
          activePage === "requestLogs" && "lg:h-screen lg:overflow-hidden",
        )}
      >
        <aside className="border-b border-slate-200 bg-white transition-colors dark:border-slate-800 dark:bg-slate-900 lg:sticky lg:top-0 lg:h-screen lg:border-r lg:border-b-0">
        <div className="flex h-full flex-col gap-5 p-4 lg:p-5">
          <div className="flex items-center gap-3 px-2 py-1">
            <img className="size-9 shrink-0" src={tokenGatewayLogo} alt="" aria-hidden="true" />
            <h1 className="text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100">{panelLabel}</h1>
          </div>
          <nav
            className="relative flex gap-1 overflow-x-auto pb-1 lg:grid lg:overflow-visible lg:pb-0"
            aria-label={`${panelLabel}页面`}
          >
            <span
              className={cx(
                "pointer-events-none absolute inset-x-0 top-0 hidden h-10 rounded-lg bg-indigo-50 ring-1 ring-inset ring-indigo-200 transition-[translate] duration-300 ease-out dark:bg-indigo-950/70 dark:ring-indigo-800 lg:block",
                sideNavIndicatorPositions[activeRouteIndex] ?? sideNavIndicatorPositions[0],
              )}
              aria-hidden="true"
            />
            {routes.map((route) => {
              const selected = activePage === route.page;
              return (
                <a
                  key={route.page}
                  className={cx(
                    "group/nav relative z-10 flex min-h-10 shrink-0 items-center rounded-lg px-3 py-2 text-sm font-medium transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/30 lg:w-full",
                    selected
                      ? "bg-indigo-50 text-indigo-800 ring-1 ring-inset ring-indigo-200 dark:bg-indigo-950/70 dark:text-indigo-300 dark:ring-indigo-800 lg:bg-transparent lg:ring-0 dark:lg:bg-transparent"
                      : "text-slate-600 hover:text-slate-950 dark:text-slate-400 dark:hover:text-slate-100",
                  )}
                  href={route.path}
                  aria-current={selected ? "page" : undefined}
                  onClick={(event) => {
                    event.preventDefault();
                    onNavigate(route.path);
                  }}
                >
                  <span className="flex items-center gap-2.5 transition-transform duration-200 group-hover/nav:-translate-y-px">
                    <RouteIcon page={route.page} />
                    <span>{route.label}</span>
                  </span>
                </a>
              );
            })}
          </nav>
        </div>
        </aside>

        <div
          className={cx(
            "min-w-0 p-4 sm:p-6",
            activePage === "requestLogs" && "lg:flex lg:h-screen lg:flex-col lg:overflow-hidden",
          )}
        >
        <header className="mb-5 flex flex-wrap items-center justify-between gap-4">
          <h1 className="text-2xl font-semibold tracking-tight text-slate-950 dark:text-slate-100">{activeRoute.label}</h1>
          <div className="flex flex-wrap items-center justify-end gap-2">
            <button
              className={`${iconButton} shrink-0`}
              type="button"
              onClick={onRefresh}
              disabled={refreshing}
              title="刷新"
              aria-label={refreshing ? "正在刷新" : "刷新"}
            >
              {refreshing ? <Loader2 className={spinnerClass} size={17} /> : <RefreshCw size={17} />}
            </button>
            <button
              className={`${iconButton} shrink-0`}
              type="button"
              onClick={onToggleTheme}
              title={theme === "light" ? "切换到深色主题" : "切换到浅色主题"}
              aria-label={theme === "light" ? "切换到深色主题" : "切换到浅色主题"}
            >
              {theme === "light" ? <Moon size={17} /> : <Sun size={17} />}
            </button>
            <button
              className={`${iconButton} shrink-0`}
              type="button"
              onClick={onLogout}
              title="退出登录"
              aria-label="退出登录"
            >
              <LogOut size={17} />
            </button>
            <div
              className="inline-flex h-9 max-w-48 items-center rounded-lg border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200"
              title={currentUser.email}
            >
              <span className="truncate">{currentUser.username}</span>
            </div>
          </div>
        </header>

        {children}
        </div>
      </main>
      {overlays}
    </>
  );
}

function RouteIcon({ page }: { page: DashboardPage }) {
  switch (page) {
    case "accounts":
      return <ServerCog size={18} />;
    case "plugins":
      return <PlugZap size={18} />;
    case "users":
      return <UsersRound size={18} />;
    case "usage":
      return <ChartNoAxesCombined size={18} />;
    case "apiKeys":
      return <KeyRound size={18} />;
    case "requestLogs":
      return <ScrollText size={18} />;
  }
}
