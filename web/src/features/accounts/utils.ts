import { errorMessageFrom } from "../../lib/errors";
import type {
  GptAccountQuotaResponse,
  GptCreditsSnapshot,
  GptQuotaSnapshot,
  OverrideEntry,
} from "../../types";

export function enabledToggleLabel(enabled: boolean) {
  return enabled ? "禁用" : "启用";
}

export function parseModelList(value: string) {
  return Array.from(
    new Set(
      value
        .split(/[,\n]/)
        .map((model) => model.trim())
        .filter(Boolean),
    ),
  );
}

export function overrideEntriesFromObject(object: Record<string, unknown>) {
  return Object.entries(object).map(([key, value]) => createOverrideEntry(key, JSON.stringify(value)));
}

export function overrideEntriesToObject(rows: OverrideEntry[], section: "header" | "body") {
  // 使用无原型对象，确保管理员输入 `__proto__` 等合法 JSON key 时只作为数据字段，
  // 不会触发 JavaScript 对象原型 setter。
  const payload: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
  const keys = new Set<string>();

  for (const row of rows) {
    const key = row.key.trim();
    const rawValue = row.value.trim();
    if (!key && !rawValue) {
      continue;
    }
    if (!key) {
      throw new Error("覆盖项 key 不能为空。");
    }
    if (keys.has(key)) {
      throw new Error(`覆盖项 key 重复: ${key}`);
    }
    keys.add(key);

    try {
      payload[key] = JSON.parse(rawValue);
    } catch (error) {
      throw new Error(`${section}.${key} 不是合法 JSON 字面量: ${errorMessageFrom(error)}`);
    }
  }

  return payload;
}

export function createOverrideEntry(key: string, value: string): OverrideEntry {
  return {
    id: createClientId(),
    key,
    value,
  };
}

/** 优先使用浏览器 UUID，为仅存在于前端的临时表单项创建唯一 ID。 */
function createClientId() {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function quotaPrimarySnapshot(quota: GptAccountQuotaResponse) {
  return (
    quota.primary ??
    quota.snapshots.find((snapshot) => snapshot.limit_id === "codex") ??
    quota.snapshots[0] ??
    null
  );
}

export function quotaStatusLabel(snapshot: GptQuotaSnapshot) {
  if (snapshot.limit_reached) {
    return "额度已触顶";
  }
  if (snapshot.allowed === true) {
    return "额度可用";
  }
  if (snapshot.allowed === false) {
    return "额度不可用";
  }
  return "已刷新";
}

export function creditsLabel(credits: GptCreditsSnapshot) {
  if (credits.unlimited) {
    return "不限";
  }
  if (!credits.has_credits) {
    return "不可用";
  }
  return credits.balance ? `余额 ${credits.balance}` : "可用";
}
