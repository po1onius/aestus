import type { RequestLogRecord } from "../types";

export function formatPercent(value: number) {
  if (!Number.isFinite(value)) {
    return "未知";
  }
  const normalized = Math.max(0, Math.min(100, value));
  return `${Number.isInteger(normalized) ? normalized.toFixed(0) : normalized.toFixed(1)}%`;
}

export function todayInputValue() {
  return dateInputValue(new Date());
}

function dateInputValue(value: Date) {
  const year = value.getFullYear();
  const month = String(value.getMonth() + 1).padStart(2, "0");
  const day = String(value.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export function localDateRangeIso(dateValue: string) {
  const selectedDate = dateValue || todayInputValue();
  const start = new Date(`${selectedDate}T00:00:00`);
  const end = new Date(start);
  end.setDate(end.getDate() + 1);

  return {
    startAt: start.toISOString(),
    endAt: end.toISOString(),
  };
}

/** Token 总量由后端以字符串返回，使用 BigInt 格式化可避免大整数精度丢失。 */
export function formatTokenCount(value: string) {
  try {
    return new Intl.NumberFormat("zh-CN").format(BigInt(value));
  } catch {
    return value;
  }
}

const compactTokenFormatter = new Intl.NumberFormat("en-US", {
  notation: "compact",
  minimumFractionDigits: 1,
  maximumFractionDigits: 1,
});

/**
 * 为概览卡片生成带 Token 单位的紧凑数值，例如 1.2M Token。
 * 直接向 Intl 传递 BigInt，在后端累计值超过 JavaScript 安全整数上限时仍能保持正确量级。
 */
export function formatCompactTokenAmount(value: string) {
  try {
    return `${compactTokenFormatter.format(BigInt(value))} Token`;
  } catch {
    return `${value} Token`;
  }
}

export function formatDuration(durationMs: number | null) {
  if (durationMs === null) {
    return "未记录";
  }
  if (durationMs < 1000) {
    return `${durationMs} ms`;
  }
  return `${(durationMs / 1000).toFixed(2)} s`;
}

/**
 * 首字耗时以请求开始时间和响应开始时间的差值为准。
 * 对缺失、无法解析或时间倒序的数据返回 null，避免前端展示误导性的负数耗时。
 */
export function firstTokenDurationMs(log: RequestLogRecord) {
  if (!log.response_started_at) {
    return null;
  }

  const requestStartedAt = new Date(log.request_started_at).getTime();
  const responseStartedAt = new Date(log.response_started_at).getTime();
  if (!Number.isFinite(requestStartedAt) || !Number.isFinite(responseStartedAt)) {
    return null;
  }

  const durationMs = responseStartedAt - requestStartedAt;
  return durationMs >= 0 ? durationMs : null;
}

export function formatOptionalDateTime(value: string | null) {
  return value ? formatDateTime(value) : "未记录";
}

export function formatDateTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

/** 请求日志的开始时间需要保留秒和毫秒，便于精确定位单次调用。 */
export function formatDateTimeWithMilliseconds(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  }).format(date);
}
