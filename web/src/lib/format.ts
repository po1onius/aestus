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

/** 返回本地日期输入框使用的若干天前日期，0 表示今天。 */
export function daysAgoInputValue(daysAgo: number) {
  const date = new Date();
  date.setDate(date.getDate() - Math.max(0, Math.trunc(daysAgo)));
  return dateInputValue(date);
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

/** 将两个本地日期转换为左闭右开的 UTC ISO 区间，结束日期按完整自然日计算。 */
export function localDateIntervalIso(startDateValue: string, endDateValue: string) {
  const start = new Date(`${startDateValue}T00:00:00`);
  const end = new Date(`${endDateValue}T00:00:00`);
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
