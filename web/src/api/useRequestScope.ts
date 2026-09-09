import { useCallback, useEffect, useRef } from "react";
import { requestJson as sendJson, requestFormData as sendFormData } from "./client";
import { authTokenStorageKey } from "../config";

/** 页面卸载后终止读取并忽略迟到结果；同类查询只允许最新一次更新页面。 */
export function useRequestScope(token: string | null) {
  const lifetime = useRef<AbortController | null>(null);
  const queries = useRef(new Map<string, AbortController>());

  useEffect(() => {
    const controller = new AbortController();
    lifetime.current = controller;
    return () => {
      controller.abort();
      for (const query of queries.current.values()) query.abort();
      queries.current.clear();
      lifetime.current = null;
    };
  }, [token]);

  const isActiveAuthToken = useCallback((candidate: string | null | undefined) =>
    Boolean(lifetime.current && !lifetime.current.signal.aborted) &&
    candidate === token && (!token || localStorage.getItem(authTokenStorageKey) === token),
    [token]);

  const beginRequest = useCallback((key: string) => {
    queries.current.get(key)?.abort();
    const controller = new AbortController();
    queries.current.set(key, controller);
    const scope = lifetime.current;
    return {
      signal: controller.signal,
      isCurrent: () => Boolean(scope && scope === lifetime.current && !scope.signal.aborted &&
        !controller.signal.aborted && queries.current.get(key) === controller && isActiveAuthToken(token)),
    };
  }, [isActiveAuthToken, token]);

  const requestJson = useCallback(async <T,>(path: string, init?: RequestInit, candidate = token): Promise<T> => {
    const scope = lifetime.current;
    if (!scope || !isActiveAuthToken(candidate)) throw new DOMException("页面请求已失效", "AbortError");
    const signal = init?.signal ? AbortSignal.any([scope.signal, init.signal]) : scope.signal;
    const result = await sendJson<T>(path, { ...init, signal }, candidate).catch(error => {
      signal.throwIfAborted();
      throw error;
    });
    signal.throwIfAborted();
    if (!isActiveAuthToken(candidate)) throw new DOMException("登录会话已切换", "AbortError");
    return result;
  }, [isActiveAuthToken, token]);

  const requestFormData = useCallback(async <T,>(path: string, body: FormData, init?: Omit<RequestInit, "body">, candidate = token): Promise<T> => {
    const scope = lifetime.current;
    if (!scope || !isActiveAuthToken(candidate)) throw new DOMException("页面请求已失效", "AbortError");
    const signal = init?.signal ? AbortSignal.any([scope.signal, init.signal]) : scope.signal;
    const result = await sendFormData<T>(path, body, { ...init, signal }, candidate).catch(error => {
      signal.throwIfAborted();
      throw error;
    });
    signal.throwIfAborted();
    if (!isActiveAuthToken(candidate)) throw new DOMException("登录会话已切换", "AbortError");
    return result;
  }, [isActiveAuthToken, token]);

  return { requestJson, requestFormData, isActiveAuthToken, beginRequest };
}
