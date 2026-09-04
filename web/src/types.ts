/** Dashboard 前后端共享响应模型。按领域集中导出，避免页面组件重复声明接口。 */
export type AccountStatus = "valid" | "unauthorized" | "invalid" | string;
export type UserRole = "platform_admin" | "tenant_owner" | "tenant_user" | string;
export type RuntimeViewState =
  | "missing"
  | "ready"
  | "token_refresh_pending"
  | "quota_limited"
  | "pending_probe"
  | "not_runtime";
export type DashboardPage =
  | "tenants"
  | "accounts"
  | "plugins"
  | "users"
  | "usage"
  | "apiKeys"
  | "requestLogs";
export type DashboardTheme = "light" | "dark";
export type AccountImportMode = "oauth" | "refreshToken";
export type AccountProviderKey = "gpt" | "claude" | "grok";
export type ProviderCredentialTab = "accounts" | "officialKeys";
export type UpstreamApiKeyProvider = "gpt" | "claude";
export type GroupPermission =
  | "account.view"
  | "account.quota.view"
  | "account.reset.view"
  | "account.reset.consume"
  | "account.override.view"
  | "account.override.update"
  | "official_api_key.view"
  | "official_api_key.override.view"
  | "official_api_key.override.update";

export interface UserGroupGrant {
  group_id: string;
  permissions: GroupPermission[];
}

export interface ProviderGroupReference {
  id: string;
  provider: UpstreamApiKeyProvider;
  name: string;
  enabled: boolean;
  created_at: string;
  updated_at: string;
  disabled_at: string | null;
}

/** 分组管理与创建选项额外携带模型集合，资源列表只需要基础分组引用。 */
export interface ProviderGroup extends ProviderGroupReference {
  allowed_models: string[];
}

export interface ProviderGroupCounts {
  account_count: number;
  upstream_api_key_count: number;
  gateway_api_key_count: number;
  enabled_gateway_api_key_count: number;
}

export interface ProviderGroupSummary extends ProviderGroup {
  counts: ProviderGroupCounts;
}

export interface DeleteProviderGroupResponse {
  id: string;
  provider: UpstreamApiKeyProvider;
  name: string;
  released_account_count: number;
  released_upstream_api_key_count: number;
  deleted_gateway_api_key_count: number;
}

/** 创建分组时可选择的未分组上游资源；不包含任何长期凭证。 */
export interface UnassignedProviderResource {
  id: string;
  resource_type: "account" | "api_key";
  display_name: string;
  detail: string;
}

export interface DashboardRoute {
  page: DashboardPage;
  path: string;
  label: string;
  platformOnly?: boolean;
  ownerOnly?: boolean;
  tenantOnly?: boolean;
}

export interface DashboardTenant {
  id: string;
  name: string;
}

export interface TenantSummary extends DashboardTenant {
  enabled: boolean;
  created_by: string;
  created_at: string;
  updated_at: string;
  disabled_at: string | null;
  code: string | null;
}

export type TenantResourceKind = "account" | "official_api_key";

interface TenantResourceBase {
  id: string;
  provider: string;
  enabled: boolean;
  status: AccountStatus;
  inflight_count: number;
}

export interface TenantAccountResource extends TenantResourceBase {
  resource_type: "account";
  email: string | null;
  plan: string;
  status_reason: string | null;
}

export interface TenantOfficialApiKeyResource extends TenantResourceBase {
  resource_type: "official_api_key";
  base_url: string;
  error: string | null;
}

export type TenantResource = TenantAccountResource | TenantOfficialApiKeyResource;

export interface ListTenantResourcesResponse extends ListPageResponse<TenantResource> {
}

export interface DashboardUser {
  id: string;
  tenant_id: string | null;
  username: string;
  email: string;
  role: UserRole;
  quota: number;
  max_concurrency: number | null;
  email_verified: boolean;
  enabled: boolean;
  created_at: string;
  updated_at: string;
  disabled_at: string | null;
}

export interface DashboardUserListItem extends DashboardUser {
  current_concurrency: {
    gpt: number;
    claude: number;
  };
}

export interface AuthResponse {
  token: string;
  user: DashboardUser;
  tenant: DashboardTenant | null;
  service_timezone: string;
  request_log_retention_days: number;
}

export interface MeResponse {
  user: DashboardUser;
  tenant: DashboardTenant | null;
  service_timezone: string;
  request_log_retention_days: number;
}

export interface ListPageResponse<T> {
  items: T[];
  offset: number;
  limit: number;
  next_offset: number | null;
}

export interface ListUsersResponse extends ListPageResponse<DashboardUserListItem> {
}

export interface GptAccountRuntime {
  account_id: string;
  runtime_exists: boolean;
  runtime_ready: boolean;
  inflight_count: number;
  next_token_refresh_at: string | null;
  quota_resets_at: string | null;
  token_usable: boolean;
  runtime_state: RuntimeViewState;
}

export interface GptAccount {
  id: string;
  account_id: string | null;
  client_id: string;
  email: string | null;
  plan_type: string;
  quota_resets_at: string | null;
  enabled: boolean;
  group: ProviderGroupReference | null;
  status: AccountStatus;
  status_reason: string | null;
  created_at: string;
  updated_at: string;
  override: RequestOverride | null;
  runtime: GptAccountRuntime;
}

export interface GptAccountQuotaResponse {
  account_id: string;
  chatgpt_account_id: string | null;
  plan_type: string;
  fetched_at: string;
  primary: GptQuotaSnapshot | null;
  snapshots: GptQuotaSnapshot[];
  rate_limit_reset_credits: RateLimitResetCreditsSummary | null;
  quota_limit_removed: boolean;
}

export interface GptQuotaSnapshot {
  limit_id: string;
  limit_name: string | null;
  allowed: boolean | null;
  limit_reached: boolean | null;
  primary: GptQuotaWindow | null;
  secondary: GptQuotaWindow | null;
  credits: GptCreditsSnapshot | null;
  individual_limit: GptSpendControlLimitSnapshot | null;
  plan_type: string;
  rate_limit_reached_type: string | null;
}

export interface GptQuotaWindow {
  used_percent: number;
  remaining_percent: number;
  window_minutes: number | null;
  resets_at: string | null;
  reset_after_seconds: number | null;
}

export interface GptCreditsSnapshot {
  has_credits: boolean;
  unlimited: boolean;
  balance: string | null;
}

export interface GptSpendControlLimitSnapshot {
  limit: string;
  used: string;
  remaining: string;
  used_percent: number;
  remaining_percent: number;
  resets_at: string | null;
  reset_after_seconds: number | null;
}

export interface RateLimitResetCreditsSummary {
  available_count: number;
}

/** ChatGPT 后端授予账号的一条可兑换额度重置记录。 */
export interface RateLimitResetCredit {
  id: string;
  reset_type: string;
  status: string;
  granted_at: string;
  expires_at: string | null;
  title: string | null;
  description: string | null;
}

export interface RateLimitResetCreditsResponse {
  credits: RateLimitResetCredit[];
  available_count: number;
}

export type ConsumeRateLimitResetCreditCode =
  | "reset"
  | "nothing_to_reset"
  | "no_credit"
  | "already_redeemed";

export interface ConsumeRateLimitResetCreditResponse {
  code: ConsumeRateLimitResetCreditCode;
  windows_reset: number;
}

export interface ListAccountsResponse extends ListPageResponse<GptAccount> {
}

export interface DeleteGptAccountResponse {
  id: string;
}

export interface ClaudeAccountRuntime {
  account_id: string;
  runtime_exists: boolean;
  runtime_ready: boolean;
  inflight_count: number;
  next_token_refresh_at: string | null;
  quota_resets_at: string | null;
  token_usable: boolean;
  runtime_state: RuntimeViewState;
}

export type ClaudeSubscriptionType = "max" | "pro" | "team" | "enterprise";

export interface ClaudeAccount {
  id: string;
  account_uuid: string | null;
  organization_uuid: string | null;
  email: string | null;
  display_name: string | null;
  subscription_type: ClaudeSubscriptionType | null;
  rate_limit_tier: string | null;
  has_extra_usage_enabled: boolean | null;
  billing_type: string | null;
  account_created_at: string | null;
  subscription_created_at: string | null;
  client_id: string;
  scopes: string[];
  refresh_token_expires_at: string | null;
  quota_resets_at: string | null;
  enabled: boolean;
  group: ProviderGroupReference | null;
  status: AccountStatus;
  status_reason: string | null;
  created_at: string;
  updated_at: string;
  override: RequestOverride | null;
  runtime: ClaudeAccountRuntime;
}

export interface ListClaudeAccountsResponse extends ListPageResponse<ClaudeAccount> {
}

export interface DeleteClaudeAccountResponse {
  id: string;
}

export interface ProviderUpstreamApiKeyRuntime {
  api_key_id: string;
  runtime_exists: boolean;
  runtime_ready: boolean;
  inflight_count: number;
  next_probe_at: string | null;
  runtime_state: RuntimeViewState;
}

export interface ProviderUpstreamApiKey {
  id: string;
  masked_api_key: string;
  base_url: string;
  enabled: boolean;
  group: ProviderGroupReference | null;
  error: string | null;
  override: RequestOverride | null;
  runtime: ProviderUpstreamApiKeyRuntime;
}

export interface ListProviderUpstreamApiKeysResponse extends ListPageResponse<ProviderUpstreamApiKey> {
}

export interface DeleteProviderUpstreamApiKeyResponse {
  id: string;
}

export interface ApiKey {
  id: string;
  name: string;
  api_key: string;
  enabled: boolean;
  group_authorized: boolean;
  group: ProviderGroupReference;
  group_allowed_models: string[];
  allowed_models: string[];
  plugin: PluginReleaseSummary | null;
  created_at: string;
  updated_at: string;
  disabled_at: string | null;
}

export interface DeleteApiKeyResponse {
  id: string;
}

export interface PluginReleaseSummary {
  id: string;
  suite_id: string;
  suite_name: string;
  description: string;
  provider: UpstreamApiKeyProvider;
  suite_enabled: boolean;
  version: number;
  manifest_sha256: string;
  artifacts: PluginArtifactSummary[];
  published_at: string;
}

export interface DeletePluginResponse {
  id: string;
  name: string;
  provider: UpstreamApiKeyProvider;
  deleted_release_count: number;
  deleted_artifact_count: number;
  unbound_gateway_api_key_count: number;
}

export type PluginSlot = "request" | "buffered_response" | "stream_response";

export interface PluginArtifactSummary {
  id: string;
  slot: PluginSlot;
  abi_version: number;
  wasm_sha256: string;
  wasm_size: number;
}

export interface ListApiKeysResponse extends ListPageResponse<ApiKey> {
}

export interface RequestLogRecord {
  request_id: string;
  provider: string;
  route: string;
  api_key_name: string | null;
  user_id: string | null;
  username: string | null;
  provider_group_id: string | null;
  provider_group_name: string | null;
  model: string | null;
  reasoning: string | null;
  service_tier: string | null;
  fast_mode: boolean | null;
  is_compaction: boolean | null;
  request_started_at: string;
  response_started_at: string | null;
  response_finished_at: string | null;
  duration_ms: number | null;
  input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  reasoning_output_tokens: number;
  total_tokens: number;
  status: "success" | "abnormal" | "failed";
  extra: Record<string, unknown>;
}

export interface RequestLogErrorResponse {
  kind?: unknown;
  status_code?: unknown;
  body?: unknown;
}

export interface RequestLogCursor {
  before_started_at: string;
  before_request_id: string;
}

export interface ListRequestLogsResponse {
  date: string;
  timezone: string;
  items: RequestLogRecord[];
  next_cursor: RequestLogCursor | null;
}

export type UsageScope = "current_user" | "tenant" | "all_users";

export interface UsageLifetime {
  total_tokens: string;
  request_count: string;
}

export interface UsageDailyPoint {
  date: string;
  total_tokens: string;
  request_count: string;
}

export interface UsageModelPoint {
  provider: string;
  model: string;
  total_tokens: string;
  request_count: string;
  percentage: number;
}

export interface UsageApiKeyPoint {
  name: string;
  total_tokens: string;
  request_count: string;
  percentage: number;
}

export interface UsageUserPoint {
  user_id: string;
  username: string;
  total_tokens: string;
  request_count: string;
  percentage: number;
}

export interface UsageTenantPoint {
  tenant_id: string;
  tenant_name: string;
  total_tokens: string;
  request_count: string;
  percentage: number;
}

export interface UsageResponse {
  scope: UsageScope;
  remaining_tokens: string;
  consumed_tokens: string;
  start_at: string;
  end_at: string;
  timezone: string;
  lifetime: UsageLifetime;
  daily: UsageDailyPoint[];
  models: UsageModelPoint[];
  api_keys: UsageApiKeyPoint[];
  users: UsageUserPoint[];
  tenants: UsageTenantPoint[];
  generated_at: string;
}

export interface RequestOverride {
  header: Record<string, unknown>;
  body: Record<string, unknown>;
}

export interface OverrideEntry {
  id: string;
  key: string;
  value: string;
}

export type RequestOverrideTarget =
  | { kind: "account"; item: GptAccount }
  | { kind: "claudeAccount"; item: ClaudeAccount }
  | { kind: "apiKey"; provider: UpstreamApiKeyProvider; item: ProviderUpstreamApiKey };

export interface OauthAuthorizationResponse {
  authorization_url: string;
  redirect_uri: string;
  state: string;
  expires_at: string;
}
