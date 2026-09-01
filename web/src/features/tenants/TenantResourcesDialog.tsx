import { KeyRound, Loader2, RefreshCw, UserRound } from "lucide-react";
import { useEffect, useState } from "react";
import { requestJson } from "../../api/client";
import { ListPager } from "../../components/ListPager";
import { Modal } from "../../components/Modal";
import { SlidingTabList } from "../../components/SlidingTabList";
import { StatusBadge } from "../../components/StatusBadge";
import { dashboardListPageSize, tenantsPath } from "../../config";
import { errorMessageFrom, showErrorToast } from "../../lib/errors";
import {
  buttonSecondary,
  cellMainClass,
  cellNoteClass,
  emptyStateClass,
  spinnerClass,
  tabClass,
  tabContentClass,
  tabIdleClass,
  tabSelectedClass,
  tableClass,
  tableScrollClass,
} from "../../lib/ui";
import type {
  ListTenantResourcesResponse,
  TenantAccountResource,
  TenantOfficialApiKeyResource,
  TenantResourceKind,
  TenantSummary,
} from "../../types";

interface TenantResourcesDialogProps {
  tenant: TenantSummary;
  token: string;
  onClose: () => void;
}

/** 平台管理员的租户资源审计视图；数据契约刻意小于租户 owner 的资源管理接口。 */
export function TenantResourcesDialog({ tenant, token, onClose }: TenantResourcesDialogProps) {
  const [kind, setKind] = useState<TenantResourceKind>("account");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<ListTenantResourcesResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [retrySignal, setRetrySignal] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    let active = true;
    setLoading(true);
    setError(null);

    const query = new URLSearchParams({
      kind,
      limit: String(dashboardListPageSize),
      offset: String(offset),
    });
    void requestJson<ListTenantResourcesResponse>(
      `${tenantsPath}/${tenant.id}/resources?${query.toString()}`,
      { signal: controller.signal },
      token,
    )
      .then((response) => {
        if (active) setPage(response);
      })
      .catch((requestError: unknown) => {
        if (!active || (requestError instanceof DOMException && requestError.name === "AbortError")) {
          return;
        }
        setError(requestError);
        setPage(null);
        showErrorToast("租户资源加载失败", requestError, `tenant-resources-${tenant.id}`);
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
      controller.abort();
    };
  }, [kind, offset, retrySignal, tenant.id, token]);

  function selectKind(nextKind: TenantResourceKind) {
    if (nextKind === kind) return;
    setPage(null);
    setOffset(0);
    setKind(nextKind);
  }

  const accounts = page?.items.filter(
    (resource): resource is TenantAccountResource => resource.resource_type === "account",
  ) ?? [];
  const apiKeys = page?.items.filter(
    (resource): resource is TenantOfficialApiKeyResource =>
      resource.resource_type === "official_api_key",
  ) ?? [];

  return (
    <Modal
      titleId="tenantResourcesTitle"
      title={`${tenant.name} · 租户资源`}
      description="平台只读视图仅展示资源身份、当前并发和健康状态，不包含账号凭证或官方 API Key。"
      className="max-w-5xl"
      onClose={onClose}
    >
      <div className="grid gap-4">
        <SlidingTabList count={2} selectedIndex={kind === "account" ? 0 : 1} ariaLabel="租户资源类型">
          <button
            type="button"
            role="tab"
            aria-selected={kind === "account"}
            className={`${tabClass} ${kind === "account" ? tabSelectedClass : tabIdleClass}`}
            onClick={() => selectKind("account")}
          >
            <span className={`${tabContentClass} inline-flex items-center gap-2`}>
              <UserRound size={16} />账号资源
            </span>
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={kind === "official_api_key"}
            className={`${tabClass} ${kind === "official_api_key" ? tabSelectedClass : tabIdleClass}`}
            onClick={() => selectKind("official_api_key")}
          >
            <span className={`${tabContentClass} inline-flex items-center gap-2`}>
              <KeyRound size={16} />官方 API Key
            </span>
          </button>
        </SlidingTabList>

        <div className="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-800">
          {loading && !page ? (
            <div className={emptyStateClass}>
              <Loader2 className={spinnerClass} size={22} />
              <span>正在加载租户资源</span>
            </div>
          ) : error ? (
            <div className="flex min-h-56 flex-col items-center justify-center gap-3 px-6 py-12 text-center text-sm text-slate-500 dark:text-slate-400">
              <p>{errorMessageFrom(error)}</p>
              <button className={buttonSecondary} type="button" onClick={() => setRetrySignal((value) => value + 1)}>
                <RefreshCw size={16} />重新加载
              </button>
            </div>
          ) : kind === "account" ? (
            accounts.length > 0 ? <AccountsTable accounts={accounts} /> : <EmptyResources label="该租户还没有账号资源" />
          ) : apiKeys.length > 0 ? (
            <OfficialApiKeysTable apiKeys={apiKeys} />
          ) : (
            <EmptyResources label="该租户还没有官方 API Key" />
          )}

          {page && (
            <ListPager
              offset={page.offset}
              limit={page.limit}
              itemCount={page.items.length}
              nextOffset={page.next_offset}
              loading={loading}
              label={kind === "account" ? "个账号资源" : "个官方 API Key"}
              onPageChange={setOffset}
            />
          )}
        </div>
      </div>
    </Modal>
  );
}

function AccountsTable({ accounts }: { accounts: TenantAccountResource[] }) {
  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[48rem]`}>
        <thead>
          <tr>
            <th>Provider</th>
            <th>邮箱</th>
            <th>Plan</th>
            <th>当前并发</th>
            <th>状态</th>
          </tr>
        </thead>
        <tbody>
          {accounts.map((account) => (
            <tr key={account.id}>
              <td><ProviderName provider={account.provider} /></td>
              <td>
                <div className={cellMainClass} title={account.email ?? "未返回邮箱"}>{account.email ?? "未返回邮箱"}</div>
                <p className={cellNoteClass} title={account.id}>{account.id}</p>
              </td>
              <td><div className={cellMainClass}>{account.plan === "unknown" ? "未返回" : account.plan}</div></td>
              <td><div className="text-lg font-semibold tabular-nums text-slate-900 dark:text-slate-100">{account.inflight_count}</div></td>
              <td>
                <div className="grid gap-1.5">
                  <StatusBadge status={account.status} />
                  <p className={cellNoteClass}>管理员：{account.enabled ? "已启用" : "已禁用"}</p>
                  {account.status_reason && <p className={`${cellNoteClass} max-w-48 text-red-700 dark:text-red-400`} title={account.status_reason}>{account.status_reason}</p>}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function OfficialApiKeysTable({ apiKeys }: { apiKeys: TenantOfficialApiKeyResource[] }) {
  return (
    <div className={tableScrollClass}>
      <table className={`${tableClass} min-w-[46rem]`}>
        <thead>
          <tr>
            <th>Provider</th>
            <th>Base URL</th>
            <th>当前并发</th>
            <th>状态</th>
          </tr>
        </thead>
        <tbody>
          {apiKeys.map((apiKey) => (
            <tr key={apiKey.id}>
              <td><ProviderName provider={apiKey.provider} /></td>
              <td>
                <div className={cellMainClass} title={apiKey.base_url}>{apiKey.base_url}</div>
                <p className={cellNoteClass} title={apiKey.id}>{apiKey.id}</p>
              </td>
              <td><div className="text-lg font-semibold tabular-nums text-slate-900 dark:text-slate-100">{apiKey.inflight_count}</div></td>
              <td>
                <div className="grid gap-1.5">
                  <StatusBadge status={apiKey.status} />
                  <p className={cellNoteClass}>管理员：{apiKey.enabled ? "已启用" : "已禁用"}</p>
                  {apiKey.error && <p className={`${cellNoteClass} max-w-48 text-red-700 dark:text-red-400`} title={apiKey.error}>{apiKey.error}</p>}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ProviderName({ provider }: { provider: string }) {
  return <span className="inline-flex rounded-md bg-slate-100 px-2 py-1 text-xs font-semibold uppercase text-slate-700 dark:bg-slate-800 dark:text-slate-200">{provider}</span>;
}

function EmptyResources({ label }: { label: string }) {
  return <div className={emptyStateClass}>{label}</div>;
}
