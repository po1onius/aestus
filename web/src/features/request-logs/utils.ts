import type { AccountStatus, RequestLogErrorResponse, RequestLogRecord } from "../../types";

export function requestLogAutoLoadKey(
  userId: string,
  date: string,
  nonSuccessOnly: boolean,
  tenantId: string,
) {
  return `${userId}:${date}:${nonSuccessOnly ? "non-success" : "all"}:${tenantId || "all-tenants"}`;
}

function logStatusCode(log: RequestLogRecord) {
  return numberFromUnknown(requestLogErrorResponse(log)?.status_code) ?? 0;
}

export function statusTone(log: RequestLogRecord): AccountStatus {
  return log.status;
}

export function statusLabel(log: RequestLogRecord) {
  const terminationKind = requestLogTerminationKind(log);
  if (terminationKind === "request_body_interrupted") {
    return "请求体传输中断";
  }
  if (terminationKind === "downstream_disconnected") {
    return "下游连接已断开";
  }
  if (terminationKind === "upstream_error") {
    return "上游连接异常中断";
  }
  if (terminationKind === "stream_idle_timeout") {
    return "上游流式响应超时";
  }
  if (terminationKind === "request_event_timeout") {
    return "日志事件等待超时";
  }
  if (terminationKind) {
    return "请求生命周期中断";
  }

  const statusCode = logStatusCode(log);
  if (statusCode > 0) {
    return `HTTP ${statusCode}`;
  }
  if (log.status === "failed") {
    return "错误响应已记录";
  }
  return "完成";
}

function requestLogErrorResponse(log: RequestLogRecord): RequestLogErrorResponse | null {
  const value = log.extra.error_response;
  return isRecord(value) ? value : null;
}

function requestLogTerminationKind(log: RequestLogRecord) {
  const value = log.extra.lifecycle_termination;
  return isRecord(value) ? stringFromUnknown(value.kind) : "";
}

export function requestLogErrorKind(log: RequestLogRecord) {
  return stringFromUnknown(requestLogErrorResponse(log)?.kind) || requestLogTerminationKind(log);
}

export function requestLogDetail(log: RequestLogRecord) {
  const errorResponse = requestLogErrorResponse(log);
  if (errorResponse) {
    return fullErrorBody(errorResponse) || "错误响应已记录";
  }

  const terminationKind = requestLogTerminationKind(log);
  if (terminationKind === "request_body_interrupted") {
    return "调用方在请求体完整读取前中断了连接";
  }
  if (terminationKind === "downstream_disconnected") {
    return "调用方在流式响应 EOF 前停止消费响应体";
  }
  if (terminationKind === "upstream_error") {
    return "上游字节流在 EOF 前发生读取错误";
  }
  if (terminationKind === "stream_idle_timeout") {
    return "上游 SSE 在配置的空闲期限内没有返回新字节";
  }
  if (terminationKind === "request_event_timeout") {
    return "日志 worker 在 24 小时内没有收到请求终态事件，已执行兜底收尾";
  }
  if (terminationKind) {
    return `请求生命周期终止：${terminationKind}`;
  }

  const extraDetail = fullExtra(log.extra);
  return extraDetail || "无扩展信息";
}

/**
 * 前端按各 provider 的请求语义统一展示 fast mode。
 * GPT Responses 没有独立的 fast_mode 日志字段，service_tier=priority 即代表快速通道；
 * Claude 则直接使用后端从 Messages speed 字段提取出的三态结果。
 */
export function requestLogFastMode(log: RequestLogRecord): boolean | null {
  if (log.provider === "gpt") {
    return log.service_tier === "priority";
  }
  return log.fast_mode;
}

/** 从 reasoning JSON 中提取 GPT/Codex reasoning.effort；缺失或格式异常时不展示伪造值。 */
export function requestLogEffort(log: RequestLogRecord): string | null {
  if (!log.reasoning) {
    return null;
  }

  const effort = parseJsonRecord(log.reasoning)?.effort;
  if (typeof effort !== "string") {
    return null;
  }
  const normalized = effort.trim();
  return normalized || null;
}

function fullErrorBody(errorResponse: RequestLogErrorResponse) {
  const body = stringFromUnknown(errorResponse.body).trim();
  if (!body) {
    return "";
  }

  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
}

function fullExtra(extra: Record<string, unknown>) {
  const payload = Object.fromEntries(
    Object.entries(extra).filter(
      ([key, value]) =>
        key !== "error_response" && key !== "lifecycle_termination" && value !== undefined,
    ),
  );
  if (Object.keys(payload).length === 0) {
    return "";
  }

  return JSON.stringify(payload, null, 2);
}

function parseJsonRecord(value: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(value);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringFromUnknown(value: unknown) {
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  return "";
}

function numberFromUnknown(value: unknown) {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}
