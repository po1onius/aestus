CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE tenants (
    -- 租户名就是稳定租户标识。租户规模很小，直接使用可读名称能让所有隔离字段和
    -- 运维查询保持直观；名称创建后不可修改。
    id TEXT PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    CHECK (char_length(id) BETWEEN 1 AND 128),
    CHECK (octet_length(id) <= 512),
    CHECK ((enabled AND disabled_at IS NULL) OR (NOT enabled AND disabled_at IS NOT NULL))
);

CREATE INDEX idx_tenants_enabled ON tenants (enabled);
CREATE INDEX idx_tenants_created_at_id ON tenants (created_at DESC, id DESC);

-- 租户码是平台管理员分发的明文注册码。删除记录即撤销租户码；同一个租户码可以在
-- owner 产生后继续注册普通成员。项目禁止数据库外键，tenant_id 关联由应用事务维护。
CREATE TABLE tenant_codes (
    code TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL UNIQUE,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (code <> ''),
    CHECK (octet_length(code) <= 128)
);

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT,
    username TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'tenant_user',
    quota BIGINT NOT NULL DEFAULT 0 CHECK (quota >= 0 AND quota <= 9007199254740991),
    consumed_tokens BIGINT NOT NULL DEFAULT 0 CHECK (consumed_tokens >= 0),
    max_concurrency INTEGER CHECK (max_concurrency IS NULL OR max_concurrency BETWEEN 1 AND 10000),
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    CHECK (char_length(username) BETWEEN 1 AND 32),
    CHECK (octet_length(username) <= 128),
    CHECK (username !~ '[[:space:]@[:cntrl:]]'),
    CHECK (role IN ('platform_admin', 'tenant_owner', 'tenant_user')),
    CHECK (
        (role = 'platform_admin' AND tenant_id IS NULL)
        OR (role IN ('tenant_owner', 'tenant_user') AND tenant_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX uq_users_username ON users (username);
CREATE INDEX idx_users_role ON users (role);
CREATE INDEX idx_users_tenant_id ON users (tenant_id);
CREATE UNIQUE INDEX uq_users_tenant_owner ON users (tenant_id) WHERE role = 'tenant_owner';
CREATE INDEX idx_users_enabled ON users (enabled);
CREATE INDEX idx_users_created_at_id ON users (created_at DESC, id DESC);

CREATE TABLE provider_groups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    CHECK (provider <> ''),
    CHECK (name <> ''),
    CHECK ((enabled AND disabled_at IS NULL) OR (NOT enabled AND disabled_at IS NOT NULL))
);

CREATE UNIQUE INDEX uq_provider_groups_tenant_provider_name ON provider_groups (tenant_id, provider, name);
CREATE INDEX idx_provider_groups_tenant_provider_enabled ON provider_groups (tenant_id, provider, enabled);

-- 模型名是上游协议中的外部标识，当前没有独立生命周期，因此直接作为分组模型映射的
-- 业务键。项目禁止数据库外键，分组创建及白名单整组替换必须由应用在事务中写入。
CREATE TABLE provider_group_models (
    group_id UUID NOT NULL,
    model_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, model_name),
    CHECK (model_name <> ''),
    CHECK (octet_length(model_name) <= 256)
);

CREATE INDEX idx_provider_group_models_model_name ON provider_group_models (model_name);

-- 分组授权主记录决定普通租户用户能否把自己的网关 API Key 绑定到对应分组。具体的
-- 上游资源查看和操作能力逐行保存在权限表中；tenant owner 始终由应用层隐式获得全部
-- 权限，不写入这里。项目禁止数据库外键，用户、租户和分组一致性由应用事务维护。
CREATE TABLE tenant_user_group_grants (
    tenant_id TEXT NOT NULL,
    user_id UUID NOT NULL,
    group_id UUID NOT NULL,
    granted_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, group_id)
);

CREATE INDEX idx_tenant_user_group_grants_tenant_user
    ON tenant_user_group_grants (tenant_id, user_id);
CREATE INDEX idx_tenant_user_group_grants_group
    ON tenant_user_group_grants (group_id);

CREATE TABLE tenant_user_group_permissions (
    tenant_id TEXT NOT NULL,
    user_id UUID NOT NULL,
    group_id UUID NOT NULL,
    permission TEXT NOT NULL,
    granted_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, group_id, permission),
    CHECK (permission IN (
        'account.view',
        'account.quota.view',
        'account.reset.view',
        'account.reset.consume',
        'account.override.view',
        'account.override.update',
        'official_api_key.view',
        'official_api_key.override.view',
        'official_api_key.override.update'
    ))
);

CREATE INDEX idx_tenant_user_group_permissions_tenant_user_permission
    ON tenant_user_group_permissions (tenant_id, user_id, permission, group_id);
CREATE INDEX idx_tenant_user_group_permissions_group
    ON tenant_user_group_permissions (group_id);

-- 插件套件按 Provider 挂载。套件主记录只保存稳定身份和全局启停开关；每个不可变发布
-- 版本由 request、buffered_response、stream_response 三种可空 artifact 组成。API Key
-- 固定绑定具体 release，空插槽明确表示回落到 provider 原生流程，不从旧版本隐式继承。
-- 项目统一不使用数据库外键，关联完整性由发布、绑定和请求鉴权路径显式校验。
CREATE TABLE plugin_suites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    provider TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (name <> ''),
    CHECK (octet_length(name) <= 128),
    CHECK (octet_length(description) <= 1024),
    CHECK (provider IN ('gpt', 'claude')),
    UNIQUE (tenant_id, provider, name)
);

CREATE INDEX idx_plugin_suites_tenant_provider_enabled ON plugin_suites (tenant_id, provider, enabled);

CREATE TABLE plugin_suite_releases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    suite_id UUID NOT NULL,
    version BIGINT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    created_by UUID NOT NULL,
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (version > 0),
    CHECK (manifest_sha256 ~ '^[0-9a-f]{64}$'),
    UNIQUE (suite_id, version),
    UNIQUE (suite_id, manifest_sha256)
);

CREATE INDEX idx_plugin_suite_releases_suite_published
    ON plugin_suite_releases (suite_id, version DESC);

CREATE TABLE plugin_suite_artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id UUID NOT NULL,
    slot TEXT NOT NULL,
    abi_version INTEGER NOT NULL DEFAULT 1,
    wasm_sha256 TEXT NOT NULL,
    wasm_size BIGINT NOT NULL,
    wasm_bytes BYTEA NOT NULL,
    CHECK (slot IN ('request', 'buffered_response', 'stream_response')),
    CHECK (abi_version = 1),
    CHECK (wasm_sha256 ~ '^[0-9a-f]{64}$'),
    CHECK (wasm_size > 0),
    CHECK (octet_length(wasm_bytes) > 0),
    CHECK (wasm_size = octet_length(wasm_bytes)),
    UNIQUE (release_id, slot)
);

CREATE INDEX idx_plugin_suite_artifacts_release_id
    ON plugin_suite_artifacts (release_id);

CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    user_id UUID NOT NULL,
    group_id UUID NOT NULL,
    name TEXT NOT NULL,
    api_key TEXT NOT NULL UNIQUE,
    plugin_release_id UUID,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    CHECK (api_key <> '')
);

CREATE INDEX idx_api_keys_user_id ON api_keys (user_id);
CREATE INDEX idx_api_keys_tenant_id ON api_keys (tenant_id);
CREATE INDEX idx_api_keys_group_id ON api_keys (group_id);
CREATE INDEX idx_api_keys_enabled ON api_keys (enabled);
CREATE INDEX idx_api_keys_plugin_release_id ON api_keys (plugin_release_id);
CREATE INDEX idx_api_keys_user_created_at_id ON api_keys (user_id, created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_api_keys_user_name ON api_keys (user_id, name);

-- 调用方 API Key 的模型白名单同样使用逐行映射。创建和整组替换均使用应用事务，写入
-- 时验证其为所属分组模型集合的非空子集；网关鉴权仍会实时同时检查 Key 和分组模型。
CREATE TABLE api_key_models (
    api_key_id UUID NOT NULL,
    model_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (api_key_id, model_name),
    CHECK (model_name <> ''),
    CHECK (octet_length(model_name) <= 256)
);

CREATE INDEX idx_api_key_models_model_name ON api_key_models (model_name);

CREATE TABLE provider_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    -- 上游资源可以先独立导入，之后再由创建分组或资源迁移操作绑定到至多一个分组。
    -- NULL 资源继续接受 maintenance 刷新，但不会发布到 Redis 可调度池。
    group_id UUID,
    refresh_token TEXT NOT NULL,
    access_token TEXT NOT NULL,
    credential_generation BIGINT NOT NULL DEFAULT 1,
    next_token_refresh_at TIMESTAMPTZ,
    quota_resets_at TIMESTAMPTZ,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    status TEXT NOT NULL DEFAULT 'valid',
    status_reason TEXT,
    client_id TEXT NOT NULL,
    specific JSONB NOT NULL DEFAULT '{}'::JSONB,
    "override" JSONB NOT NULL DEFAULT '{"header": {}, "body": {}}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (provider <> ''),
    CHECK (credential_generation > 0),
    CHECK (status IN ('valid', 'unauthorized', 'invalid')),
    CHECK (
        (status = 'invalid' AND next_token_refresh_at IS NULL)
        OR (status IN ('valid', 'unauthorized') AND next_token_refresh_at IS NOT NULL)
    ),
    CHECK (jsonb_typeof(specific) = 'object'),
    CHECK (jsonb_typeof("override") = 'object'),
    CHECK ("override" ? 'header'),
    CHECK ("override" ? 'body'),
    CHECK (jsonb_typeof("override"->'header') = 'object'),
    CHECK (jsonb_typeof("override"->'body') = 'object'),
    CHECK (("override" - 'header' - 'body') = '{}'::JSONB)
);

CREATE INDEX idx_provider_accounts_provider_enabled_status ON provider_accounts (provider, enabled, status);
CREATE INDEX idx_provider_accounts_tenant_provider_created_at_id ON provider_accounts (tenant_id, provider, created_at DESC, id DESC);
CREATE INDEX idx_provider_accounts_group_id ON provider_accounts (group_id);
CREATE INDEX idx_provider_accounts_provider_created_at_id ON provider_accounts (provider, created_at DESC, id DESC);
CREATE INDEX idx_provider_accounts_refresh_due ON provider_accounts (provider, status, next_token_refresh_at);
CREATE INDEX idx_provider_accounts_quota_due ON provider_accounts (provider, quota_resets_at);

-- Claude 的 account UUID 是 provider 私有身份事实，只保存在 specific 中。部分唯一表达式
-- 索引既不会把 Claude 字段提升到通用表结构，也能在多个 OAuth callback 并发入库时由
-- PostgreSQL 原子拒绝重复账号；其他 provider（尤其允许账号 ID 重复的 GPT）不受影响。
CREATE UNIQUE INDEX uq_provider_accounts_claude_account_uuid
    ON provider_accounts (tenant_id, (specific ->> 'account_uuid'))
    WHERE provider = 'claude' AND (specific ->> 'account_uuid') IS NOT NULL;

CREATE TABLE provider_api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    -- 官方 Key 与 OAuth 账号使用相同的可选单分组归属语义。
    group_id UUID,
    api_key TEXT NOT NULL,
    base_url TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    error TEXT,
    next_probe_at TIMESTAMPTZ,
    "override" JSONB NOT NULL DEFAULT '{"header": {}, "body": {}}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (provider <> ''),
    CHECK (api_key <> ''),
    CHECK (base_url <> ''),
    CHECK ((error IS NULL) = (next_probe_at IS NULL)),
    CHECK (jsonb_typeof("override") = 'object'),
    CHECK ("override" ? 'header'),
    CHECK ("override" ? 'body'),
    CHECK (jsonb_typeof("override"->'header') = 'object'),
    CHECK (jsonb_typeof("override"->'body') = 'object'),
    CHECK (("override" - 'header' - 'body') = '{}'::JSONB)
);

CREATE INDEX idx_provider_api_keys_provider_created_at_id ON provider_api_keys (provider, created_at DESC, id DESC);
CREATE INDEX idx_provider_api_keys_tenant_provider_created_at_id ON provider_api_keys (tenant_id, provider, created_at DESC, id DESC);
CREATE INDEX idx_provider_api_keys_group_id ON provider_api_keys (group_id);
CREATE INDEX idx_provider_api_keys_probe_due ON provider_api_keys (provider, next_probe_at) WHERE enabled AND next_probe_at IS NOT NULL;
