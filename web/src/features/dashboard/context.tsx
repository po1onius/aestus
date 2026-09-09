import type { Dispatch, SetStateAction } from "react";
import { createContext, useContext } from "react";
import type { DashboardTheme, DashboardUser } from "../../types";
import type { ProviderAccess } from "../group-access/access";

export interface PageProps {
  refreshRevision: number;
  onLoadingChange: (loading: boolean) => void;
}

interface DashboardContextValue {
  authToken: string;
  currentUser: DashboardUser;
  setCurrentUser: Dispatch<SetStateAction<DashboardUser>>;
  serviceTimezone: string;
  requestLogRetentionDays: number;
  theme: DashboardTheme;
  providerAccess: ProviderAccess;
}

export const DashboardContext = createContext<DashboardContextValue | null>(null);

/** 仅共享会话、显示设置和权限，页面数据与表单由各页面持有。 */
export function useDashboard() {
  const context = useContext(DashboardContext);
  if (!context) throw new Error("Dashboard 页面必须在已登录的控制台内渲染");
  return context;
}
