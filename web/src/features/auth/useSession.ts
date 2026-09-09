import type { SetStateAction } from "react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { isDashboardAuthError, requestJson, setAuthExpiredHandler } from "../../api/client";
import { authPath, authTokenStorageKey } from "../../config";
import { errorMessageFrom, showErrorToast } from "../../lib/errors";
import type { AuthResponse, DashboardUser, MeResponse } from "../../types";

export function useSession() {
  const [token, setToken] = useState(() => localStorage.getItem(authTokenStorageKey));
  const [session, setSession] = useState<(MeResponse & { token: string }) | null>(null);
  const [loading, setLoading] = useState(Boolean(token));

  const logout = useCallback(() => {
    localStorage.removeItem(authTokenStorageKey);
    setToken(null);
    setSession(null);
    setLoading(false);
  }, []);

  useEffect(() => {
    setAuthExpiredHandler((expiredToken, error) => {
      if (localStorage.getItem(authTokenStorageKey) !== expiredToken) return;
      logout();
      toast.error("登录状态已失效", { description: errorMessageFrom(error) });
    });
    return () => setAuthExpiredHandler(null);
  }, [logout]);

  useEffect(() => {
    if (!token || session?.token === token) {
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    void requestJson<MeResponse>(`${authPath}/me`, { signal: controller.signal }, token)
      .then((data) => {
        if (!controller.signal.aborted && localStorage.getItem(authTokenStorageKey) === token) {
          setSession({ ...data, token });
        }
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted && !isDashboardAuthError(error)) showErrorToast("登录状态检查失败", error);
      })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [token]);

  const applyAuth = useCallback((data: AuthResponse) => {
    localStorage.setItem(authTokenStorageKey, data.token);
    setToken(data.token);
    setSession(data);
    setLoading(false);
  }, []);

  const setCurrentUser = useCallback((value: SetStateAction<DashboardUser>) => {
    setSession((current) => current ? {
      ...current,
      user: typeof value === "function" ? value(current.user) : value,
    } : null);
  }, []);

  return { token, session, loading, applyAuth, logout, setCurrentUser };
}
