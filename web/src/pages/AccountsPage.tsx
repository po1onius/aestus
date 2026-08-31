import claudeLogo from "@lobehub/icons-static-svg/icons/claude.svg";
import grokLogo from "@lobehub/icons-static-svg/icons/grok.svg";
import openAiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import { FolderTree, Loader2, Plus, ShieldOff } from "lucide-react";
import { ListPager } from "../components/ListPager";
import { Metric } from "../components/Metric";
import { SlidingTabList } from "../components/SlidingTabList";
import { accountProviderTabs } from "../config";
import { ClaudeAccountsTable } from "../features/accounts/ClaudeAccountsTable";
import { GptAccountsTable } from "../features/accounts/GptAccountsTable";
import { ProviderGroupsTable } from "../features/accounts/ProviderGroupsTable";
import { ProviderUpstreamApiKeysTable } from "../features/accounts/ProviderUpstreamApiKeysTable";
import {
  buttonPrimary,
  buttonSecondary,
  cx,
  emptyStateClass,
  metricGridClass,
  panelClass,
  spinnerClass,
  tabClass,
  tabContentClass,
  tabIdleClass,
  tabSelectedClass,
} from "../lib/ui";
import type {
  AccountProviderKey,
  ClaudeAccount,
  GptAccount,
  GptAccountQuotaResponse,
  ProviderCredentialTab,
  ProviderGroupSummary,
  ProviderUpstreamApiKey,
  RequestOverrideTarget,
  UpstreamApiKeyProvider,
} from "../types";

interface AccountsPageProps {
  accounts: GptAccount[];
  claudeAccounts: ClaudeAccount[];
  gptUpstreamApiKeys: ProviderUpstreamApiKey[];
  claudeUpstreamApiKeys: ProviderUpstreamApiKey[];
  accountQuotas: Record<string, GptAccountQuotaResponse>;
  loading: boolean;
  activeProvider: AccountProviderKey;
  activeCredentialTab: ProviderCredentialTab;
  providerGroupsVisible: boolean;
  providerGroupsLoading: boolean;
  providerGroupSavingId: string | null;
  enabledUpdatingId: string | null;
  accountDeletingId: string | null;
  upstreamApiKeyDeletingId: string | null;
  upstreamApiKeyEnabledUpdatingId: string | null;
  quotaRefreshingIds: Record<string, boolean>;
  resetOperationAccountId: string | null;
  providerGroups: ProviderGroupSummary[];
  resourceGroupUpdatingId: string | null;
  pageOffset: number;
  pageSize: number;
  nextPageOffset: number | null;
  onProviderChange: (provider: AccountProviderKey) => void;
  onCredentialTabChange: (tab: ProviderCredentialTab) => void;
  onProviderGroupsView: () => void;
  onOpenAccountImport: () => void;
  onOpenUpstreamApiKey: () => void;
  onOpenProviderGroupCreate: () => void;
  onRenameProviderGroup: (group: ProviderGroupSummary, name: string) => Promise<boolean>;
  onEditProviderGroupModels: (group: ProviderGroupSummary) => void;
  onToggleProviderGroupEnabled: (group: ProviderGroupSummary) => Promise<boolean>;
  onDeleteProviderGroup: (group: ProviderGroupSummary) => void;
  onUpdateClaudeGroup: (account: ClaudeAccount, groupId: string) => void;
  onUpdateGptGroup: (account: GptAccount, groupId: string) => void;
  onUpdateUpstreamApiKeyGroup: (
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
    groupId: string,
  ) => void;
  onUpdateClaudeEnabled: (account: ClaudeAccount, enabled: boolean) => void;
  onUpdateGptEnabled: (account: GptAccount, enabled: boolean) => void;
  onUpdateUpstreamApiKeyEnabled: (
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
    enabled: boolean,
  ) => void;
  onRefreshAccountQuota: (account: GptAccount) => void;
  onOpenRateLimitReset: (account: GptAccount) => void;
  onDeleteGptAccount: (account: GptAccount) => void;
  onDeleteClaudeAccount: (account: ClaudeAccount) => void;
  onDeleteUpstreamApiKey: (
    provider: UpstreamApiKeyProvider,
    apiKey: ProviderUpstreamApiKey,
  ) => void;
  onOpenRequestOverride: (target: RequestOverrideTarget) => void;
  onPageChange: (offset: number) => void;
}

const providerLogos: Record<AccountProviderKey, string> = {
  gpt: openAiLogo,
  claude: claudeLogo,
  grok: grokLogo,
};

/**
 * Provider 资源总览页面。
 * 账号和官方 Key 使用既有凭证 Tab；分组通过相邻的独立按钮切换主表内容，不混入 Tab
 * 的凭证类型语义。官方 API Key 的展示和操作继续保持 provider 中立。
 */
export function AccountsPage({
  accounts,
  claudeAccounts,
  gptUpstreamApiKeys,
  claudeUpstreamApiKeys,
  accountQuotas,
  loading,
  activeProvider,
  activeCredentialTab,
  providerGroupsVisible,
  providerGroupsLoading,
  providerGroupSavingId,
  enabledUpdatingId,
  accountDeletingId,
  upstreamApiKeyDeletingId,
  upstreamApiKeyEnabledUpdatingId,
  quotaRefreshingIds,
  resetOperationAccountId,
  providerGroups,
  resourceGroupUpdatingId,
  pageOffset,
  pageSize,
  nextPageOffset,
  onProviderChange,
  onCredentialTabChange,
  onProviderGroupsView,
  onOpenAccountImport,
  onOpenUpstreamApiKey,
  onOpenProviderGroupCreate,
  onRenameProviderGroup,
  onEditProviderGroupModels,
  onToggleProviderGroupEnabled,
  onDeleteProviderGroup,
  onUpdateClaudeGroup,
  onUpdateGptGroup,
  onUpdateUpstreamApiKeyGroup,
  onUpdateClaudeEnabled,
  onUpdateGptEnabled,
  onUpdateUpstreamApiKeyEnabled,
  onRefreshAccountQuota,
  onOpenRateLimitReset,
  onDeleteGptAccount,
  onDeleteClaudeAccount,
  onDeleteUpstreamApiKey,
  onOpenRequestOverride,
  onPageChange,
}: AccountsPageProps) {
  const activeProviderMeta =
    accountProviderTabs.find((provider) => provider.key === activeProvider) ?? accountProviderTabs[0];
  const activeAccounts =
    activeProvider === "gpt" ? accounts : activeProvider === "claude" ? claudeAccounts : [];
  const activeUpstreamApiKeys =
    activeProvider === "gpt"
      ? gptUpstreamApiKeys
      : activeProvider === "claude"
        ? claudeUpstreamApiKeys
        : [];
  const activeProviderResourceCount = activeAccounts.length + activeUpstreamApiKeys.length;
  const activeCount =
    activeAccounts.filter((account) => account.enabled).length +
    activeUpstreamApiKeys.filter((apiKey) => apiKey.enabled).length;
  const unhealthyCount =
    activeAccounts.filter((account) => account.status !== "valid").length +
    activeUpstreamApiKeys.filter((apiKey) => apiKey.runtime.next_probe_at !== null).length;
  const runtimeReadyCount =
    activeAccounts.filter((account) => account.runtime.runtime_ready).length +
    activeUpstreamApiKeys.filter((apiKey) => apiKey.runtime.runtime_ready).length;
  const activeApiKeyProvider = asUpstreamApiKeyProvider(activeProvider);
  const activeProviderGroups = activeApiKeyProvider
    ? providerGroups.filter((group) => group.provider === activeApiKeyProvider)
    : [];
  const activeEnabledProviderGroups = activeProviderGroups.filter((group) => group.enabled);
  const accountsSelected = !providerGroupsVisible && activeCredentialTab === "accounts";
  const officialKeysSelected = !providerGroupsVisible && activeCredentialTab === "officialKeys";

  return (
    <section className="min-w-0">
      <div className={`${panelClass} overflow-hidden`}>
        <SlidingTabList
          count={3}
          selectedIndex={accountProviderTabs.findIndex((provider) => provider.key === activeProvider)}
          ariaLabel="账号 Provider"
          className="rounded-none border-x-0 border-t-0"
        >
          {accountProviderTabs.map((provider) => (
            <button
              key={provider.key}
              type="button"
                  className={cx(
                    "group/provider relative z-10 flex min-h-11 items-center justify-between gap-3 rounded-md px-3 py-2 text-sm font-semibold transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/30",
                    activeProvider === provider.key
                      ? "text-indigo-800 dark:text-indigo-300"
                      : "text-slate-600 hover:text-slate-950 dark:text-slate-400 dark:hover:text-slate-100",
              )}
              onClick={() => onProviderChange(provider.key)}
              role="tab"
              aria-selected={activeProvider === provider.key}
            >
              <span className="flex w-full items-center justify-between gap-3 transition-transform duration-200 group-hover/provider:-translate-y-px">
                <span className="flex min-w-0 items-center gap-2.5">
                  <img className="size-5 shrink-0 opacity-80" src={providerLogos[provider.key]} alt="" aria-hidden="true" />
                  <span>{provider.label}</span>
                </span>
                <strong className="inline-flex min-w-7 items-center justify-center rounded-full bg-slate-100 px-2 py-0.5 text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
                  {providerResourceCount(
                    provider.key,
                    accounts,
                    claudeAccounts,
                    gptUpstreamApiKeys,
                    claudeUpstreamApiKeys,
                  )}
                </strong>
              </span>
            </button>
          ))}
        </SlidingTabList>

        <section className={`${metricGridClass} border-b border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900`} aria-label={`${activeProviderMeta.label} 数据概览`}>
          <Metric label="当前已加载" value={activeProviderResourceCount.toString()} />
          <Metric label="启用" value={activeCount.toString()} tone="good" />
          <Metric label="就绪" value={runtimeReadyCount.toString()} tone="good" />
          <Metric label="异常" value={unhealthyCount.toString()} tone="warn" />
        </section>

        {activeProviderMeta.ready && activeApiKeyProvider && (
          <div className="flex flex-col gap-3 border-b border-slate-200 px-4 py-3 dark:border-slate-800 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex w-full flex-col gap-2 sm:max-w-lg sm:flex-row sm:items-center">
              <SlidingTabList
                count={2}
                selectedIndex={providerGroupsVisible ? -1 : officialKeysSelected ? 1 : 0}
                ariaLabel={`${activeProviderMeta.label} 凭证类型`}
                className="w-full sm:max-w-sm sm:flex-1"
              >
                <button
                  className={cx(tabClass, accountsSelected ? tabSelectedClass : tabIdleClass)}
                  type="button"
                  onClick={() => onCredentialTabChange("accounts")}
                  role="tab"
                  aria-selected={accountsSelected}
                >
                  <span className={tabContentClass}>OAuth 账号</span>
                </button>
                <button
                  className={cx(tabClass, officialKeysSelected ? tabSelectedClass : tabIdleClass)}
                  type="button"
                  onClick={() => onCredentialTabChange("officialKeys")}
                  role="tab"
                  aria-selected={officialKeysSelected}
                >
                  <span className={tabContentClass}>官方 Key</span>
                </button>
              </SlidingTabList>
              <button
                type="button"
                className={providerGroupsVisible ? buttonPrimary : buttonSecondary}
                onClick={onProviderGroupsView}
                aria-pressed={providerGroupsVisible}
                title={`查看 ${activeProviderMeta.label} 分组`}
              >
                <FolderTree size={18} />
                分组
              </button>
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className={buttonPrimary}
                disabled={providerGroupsVisible && Boolean(providerGroupSavingId)}
                onClick={
                  providerGroupsVisible
                    ? onOpenProviderGroupCreate
                    : officialKeysSelected
                      ? onOpenUpstreamApiKey
                      : onOpenAccountImport
                }
                title={
                  providerGroupsVisible
                    ? `添加 ${activeProviderMeta.label} 分组`
                    : officialKeysSelected
                      ? `添加 ${activeProviderMeta.label} 官方 Key`
                      : `导入 ${activeProviderMeta.label} OAuth 账号`
                }
              >
                <Plus size={18} />
                添加
              </button>
            </div>
          </div>
        )}

        {!activeProviderMeta.ready ? (
          <div className={`${emptyStateClass} flex-col`}>
            <ShieldOff size={24} />
            <div>
              <span className="font-semibold text-slate-700 dark:text-slate-300">{activeProviderMeta.label} Provider 预留</span>
              <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">当前 provider 后端能力接入后会在这里展示账号、额度和调度状态。</p>
            </div>
          </div>
        ) : providerGroupsVisible && activeApiKeyProvider ? (
          providerGroupsLoading ? (
            <div className={emptyStateClass}>
              <Loader2 className={spinnerClass} size={24} />
              <span>正在加载分组</span>
            </div>
          ) : (
            <ProviderGroupsTable
              groups={activeProviderGroups}
              savingId={providerGroupSavingId}
              onRename={onRenameProviderGroup}
              onEditModels={onEditProviderGroupModels}
              onToggleEnabled={onToggleProviderGroupEnabled}
              onDelete={onDeleteProviderGroup}
            />
          )
        ) : loading ? (
          <div className={emptyStateClass}>
            <Loader2 className={spinnerClass} size={24} />
            <span>正在加载凭证</span>
          </div>
        ) : officialKeysSelected && activeApiKeyProvider ? (
          activeUpstreamApiKeys.length === 0 ? (
            <EmptyCredentials label={`还没有添加 ${activeProviderMeta.label} 官方 Key`} />
          ) : (
            <ProviderUpstreamApiKeysTable
              provider={activeApiKeyProvider}
              providerLabel={activeProviderMeta.label}
              apiKeys={activeUpstreamApiKeys}
              groups={activeEnabledProviderGroups}
              groupUpdatingId={resourceGroupUpdatingId}
              enabledUpdatingId={upstreamApiKeyEnabledUpdatingId}
              deletingId={upstreamApiKeyDeletingId}
              onUpdateEnabled={(apiKey, enabled) =>
                onUpdateUpstreamApiKeyEnabled(activeApiKeyProvider, apiKey, enabled)
              }
              onUpdateGroup={(apiKey, groupId) =>
                onUpdateUpstreamApiKeyGroup(activeApiKeyProvider, apiKey, groupId)
              }
              onOpenOverride={(apiKey) =>
                onOpenRequestOverride({ kind: "apiKey", provider: activeApiKeyProvider, item: apiKey })
              }
              onDelete={(apiKey) => onDeleteUpstreamApiKey(activeApiKeyProvider, apiKey)}
            />
          )
        ) : activeProvider === "claude" ? (
          claudeAccounts.length === 0 ? (
            <EmptyCredentials label="还没有导入 Claude OAuth 账号" />
          ) : (
            <ClaudeAccountsTable
              accounts={claudeAccounts}
              groups={activeEnabledProviderGroups}
              groupUpdatingId={resourceGroupUpdatingId}
              enabledUpdatingId={enabledUpdatingId}
              deletingId={accountDeletingId}
              onUpdateEnabled={onUpdateClaudeEnabled}
              onUpdateGroup={onUpdateClaudeGroup}
              onOpenOverride={(account) =>
                onOpenRequestOverride({ kind: "claudeAccount", item: account })
              }
              onDelete={onDeleteClaudeAccount}
            />
          )
        ) : accounts.length === 0 ? (
          <EmptyCredentials label="还没有导入账号" />
        ) : (
          <GptAccountsTable
            accounts={accounts}
            quotas={accountQuotas}
            quotaRefreshingIds={quotaRefreshingIds}
            resetOperationAccountId={resetOperationAccountId}
            groups={activeEnabledProviderGroups}
            groupUpdatingId={resourceGroupUpdatingId}
            enabledUpdatingId={enabledUpdatingId}
            deletingId={accountDeletingId}
            onRefreshQuota={onRefreshAccountQuota}
            onOpenRateLimitReset={onOpenRateLimitReset}
            onUpdateEnabled={onUpdateGptEnabled}
            onUpdateGroup={onUpdateGptGroup}
            onOpenOverride={(account) => onOpenRequestOverride({ kind: "account", item: account })}
            onDelete={onDeleteGptAccount}
          />
        )}
        {activeProviderMeta.ready && activeApiKeyProvider && !providerGroupsVisible && (
          <ListPager
            offset={pageOffset}
            limit={pageSize}
            itemCount={officialKeysSelected ? activeUpstreamApiKeys.length : activeAccounts.length}
            nextOffset={nextPageOffset}
            loading={loading}
            label="项凭证"
            onPageChange={onPageChange}
          />
        )}
      </div>
    </section>
  );
}

function EmptyCredentials({ label }: { label: string }) {
  return (
    <div className={emptyStateClass}>
      <ShieldOff size={24} />
      <span>{label}</span>
    </div>
  );
}

function asUpstreamApiKeyProvider(
  provider: AccountProviderKey,
): UpstreamApiKeyProvider | null {
  return provider === "gpt" || provider === "claude" ? provider : null;
}

function providerResourceCount(
  provider: AccountProviderKey,
  accounts: GptAccount[],
  claudeAccounts: ClaudeAccount[],
  gptUpstreamApiKeys: ProviderUpstreamApiKey[],
  claudeUpstreamApiKeys: ProviderUpstreamApiKey[],
) {
  switch (provider) {
    case "gpt":
      return accounts.length + gptUpstreamApiKeys.length;
    case "claude":
      return claudeAccounts.length + claudeUpstreamApiKeys.length;
    default:
      return 0;
  }
}
