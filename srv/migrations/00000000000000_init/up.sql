CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    quota BIGINT NOT NULL DEFAULT 0 CHECK (quota >= 0 AND quota <= 9007199254740991),
    consumed_tokens BIGINT NOT NULL DEFAULT 0 CHECK (consumed_tokens >= 0),
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    CHECK (char_length(username) BETWEEN 1 AND 32),
    CHECK (octet_length(username) <= 128),
    CHECK (username !~ '[[:space:]@[:cntrl:]]')
);

CREATE UNIQUE INDEX uq_users_username ON users (username);
CREATE INDEX idx_users_role ON users (role);
CREATE INDEX idx_users_enabled ON users (enabled);
CREATE INDEX idx_users_created_at_id ON users (created_at DESC, id DESC);

CREATE TABLE provider_groups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
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

CREATE UNIQUE INDEX uq_provider_groups_provider_name ON provider_groups (provider, name);
CREATE INDEX idx_provider_groups_provider_enabled ON provider_groups (provider, enabled);

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

-- 插件套件按 Provider 挂载。套件主记录只保存稳定身份和全局启停开关；每个不可变发布
-- 版本由 request、buffered_response、stream_response 三种可空 artifact 组成。API Key
-- 固定绑定具体 release，空插槽明确表示回落到 provider 原生流程，不从旧版本隐式继承。
-- 项目统一不使用数据库外键，关联完整性由发布、绑定和请求鉴权路径显式校验。
CREATE TABLE plugin_suites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
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
    UNIQUE (provider, name)
);

CREATE INDEX idx_plugin_suites_provider_enabled ON plugin_suites (provider, enabled);

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
CREATE INDEX idx_provider_accounts_group_id ON provider_accounts (group_id);
CREATE INDEX idx_provider_accounts_provider_created_at_id ON provider_accounts (provider, created_at DESC, id DESC);
CREATE INDEX idx_provider_accounts_refresh_due ON provider_accounts (provider, status, next_token_refresh_at);
CREATE INDEX idx_provider_accounts_quota_due ON provider_accounts (provider, quota_resets_at);

-- Claude 的 account UUID 是 provider 私有身份事实，只保存在 specific 中。部分唯一表达式
-- 索引既不会把 Claude 字段提升到通用表结构，也能在多个 OAuth callback 并发入库时由
-- PostgreSQL 原子拒绝重复账号；其他 provider（尤其允许账号 ID 重复的 GPT）不受影响。
CREATE UNIQUE INDEX uq_provider_accounts_claude_account_uuid
    ON provider_accounts ((specific ->> 'account_uuid'))
    WHERE provider = 'claude' AND (specific ->> 'account_uuid') IS NOT NULL;

CREATE TABLE provider_api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
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
CREATE INDEX idx_provider_api_keys_group_id ON provider_api_keys (group_id);
CREATE INDEX idx_provider_api_keys_probe_due ON provider_api_keys (provider, next_probe_at) WHERE enabled AND next_probe_at IS NOT NULL;
