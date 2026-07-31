interface ApiErrorBody {
  error?: {
    code?: unknown;
    message?: unknown;
    details?: unknown;
  };
}

type AuthExpiredHandler = (token: string, error: ApiRequestError) => void;

let authExpiredHandler: AuthExpiredHandler | null = null;

/** 保留后端稳定错误契约，调用方可依据状态码/错误码处理，而不需要解析展示文案。 */
export class ApiRequestError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: Record<string, unknown> | null;
  readonly requestId: string | null;

  constructor(options: {
    status: number;
    code: string;
    message: string;
    details?: unknown;
    requestId: string | null;
  }) {
    super(options.message);
    this.name = "ApiRequestError";
    this.status = options.status;
    this.code = options.code;
    this.details = isRecord(options.details) ? options.details : null;
    this.requestId = options.requestId;
  }
}

/** App 在一个位置登记登录失效处理，所有带 Dashboard token 的请求共享该行为。 */
export function setAuthExpiredHandler(handler: AuthExpiredHandler | null) {
  authExpiredHandler = handler;
}

export function isDashboardAuthError(error: unknown): error is ApiRequestError {
  return (
    error instanceof ApiRequestError &&
    error.status === 401 &&
    (error.code === "missing_dashboard_token" || error.code === "invalid_dashboard_token")
  );
}

export function isProviderStateSyncError(error: unknown): error is ApiRequestError {
  return error instanceof ApiRequestError && error.code === "provider_state_sync_failed";
}

/** Dashboard JSON 请求的唯一出口，统一认证头、错误契约和非 JSON fallback 诊断。 */
export async function requestJson<T>(
  path: string,
  init?: RequestInit,
  token?: string | null,
): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(init?.headers ?? {}),
    },
  });
  return parseJsonResponse<T>(response, token);
}

/** multipart 上传不能手动设置 Content-Type，否则浏览器生成的 boundary 会丢失。 */
export async function requestFormData<T>(
  path: string,
  formData: FormData,
  init?: Omit<RequestInit, "body">,
  token?: string | null,
): Promise<T> {
  const response = await fetch(path, {
    ...init,
    body: formData,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(init?.headers ?? {}),
    },
  });
  return parseJsonResponse<T>(response, token);
}

async function parseJsonResponse<T>(response: Response, token?: string | null): Promise<T> {
  const requestId = response.headers.get("x-request-id");
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("application/json")) {
    const body = await response.text().catch(() => "");
    const preview = body.trim().slice(0, 120);
    const error = new ApiRequestError({
      status: response.status,
      code: "non_json_response",
      message: preview
        ? `接口返回非 JSON 响应: HTTP ${response.status}, body=${preview}`
        : `接口返回非 JSON 响应: HTTP ${response.status}`,
      requestId,
    });
    notifyAuthExpired(token, error);
    throw error;
  }

  let payload: T | ApiErrorBody;
  try {
    payload = (await response.json()) as T | ApiErrorBody;
  } catch {
    const error = new ApiRequestError({
      status: response.status,
      code: "invalid_json_response",
      message: `接口返回了无法解析的 JSON: HTTP ${response.status}`,
      requestId,
    });
    notifyAuthExpired(token, error);
    throw error;
  }

  if (!response.ok) {
    const body = payload as ApiErrorBody;
    const error = new ApiRequestError({
      status: response.status,
      code: typeof body.error?.code === "string" ? body.error.code : "request_failed",
      message:
        typeof body.error?.message === "string"
          ? body.error.message
          : `请求失败: HTTP ${response.status}`,
      details: body.error?.details,
      requestId,
    });
    notifyAuthExpired(token, error);
    throw error;
  }

  return payload as T;
}

function notifyAuthExpired(token: string | null | undefined, error: ApiRequestError) {
  if (token && isDashboardAuthError(error)) {
    authExpiredHandler?.(token, error);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
