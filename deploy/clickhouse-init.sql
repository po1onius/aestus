CREATE TABLE IF NOT EXISTS gateway_request_logs
(
    request_id UUID,
    -- worker 最后收到的上游账号或官方 API Key 的内部 UUID；未收到时为空。
    resource_id Nullable(UUID),
    provider LowCardinality(String),
    route LowCardinality(String),
    api_key_name Nullable(String),
    tenant_id Nullable(String),
    user_id Nullable(UUID),
    username Nullable(String),
    provider_group_id Nullable(UUID),
    provider_group_name Nullable(String),
    model Nullable(String),
    reasoning Nullable(String),
    service_tier Nullable(String),
    fast_mode Nullable(Bool),
    is_compaction Nullable(Bool),
    -- 由网关按照 AESTUS_TIMEZONE 在写入边界一次性计算。日聚合只读取该稳定日期，避免
    -- 查询方时区变化或 ClickHouse 服务器时区影响已经落盘的业务日语义。
    usage_date Date,
    request_started_at DateTime64(3, 'UTC'),
    response_started_at Nullable(DateTime64(3, 'UTC')),
    response_finished_at Nullable(DateTime64(3, 'UTC')),
    input_tokens Int64,
    cached_input_tokens Int64,
    output_tokens Int64,
    reasoning_output_tokens Int64,
    total_tokens Int64,
    status LowCardinality(String),
    extra String,
    -- 主表继续服务平台管理员全局时间线；两个轻量投影分别为普通用户与租户 owner 的
    -- 等值过滤提供独立稀疏索引，不复制 extra 等大字段。
    PROJECTION by_user INDEX user_id TYPE basic,
    PROJECTION by_tenant INDEX tenant_id TYPE basic
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(request_started_at)
-- 日志页按时间范围和请求 ID 做 keyset 分页，排序键优先放时间列，减少当天日志
-- 翻页时的扫描范围；用户维度由 by_user 轻量投影与时间主索引共同完成裁剪。
ORDER BY (request_started_at, request_id)
-- 明细日志只承担近期诊断。初始化使用 30 天默认值，网关启动后会按
-- AESTUS_REQUEST_LOG_RETENTION_DAYS 自动同步 TTL。TTL 删除不会触发下方物化视图的
-- 反向扣减，因此长期统计会独立保留；物理删除由 ClickHouse 后台 merge 异步完成。
TTL request_started_at + INTERVAL 30 DAY DELETE;

CREATE TABLE IF NOT EXISTS gateway_request_usage_daily
(
    usage_date Date,
    tenant_id String,
    user_id UUID,
    provider LowCardinality(String),
    model LowCardinality(String),
    api_key_name LowCardinality(String),
    input_tokens Int64,
    cached_input_tokens Int64,
    output_tokens Int64,
    reasoning_output_tokens Int64,
    total_tokens Int64,
    request_count UInt64,
    success_count UInt64,
    abnormal_count UInt64,
    failed_count UInt64
)
ENGINE = SummingMergeTree((
    input_tokens,
    cached_input_tokens,
    output_tokens,
    reasoning_output_tokens,
    total_tokens,
    request_count,
    success_count,
    abnormal_count,
    failed_count
))
PARTITION BY toYYYYMM(usage_date)
-- tenant_id 先隔离租户范围，租户内再按 user_id 排序；平台管理员扫描的也是已经大幅
-- 压缩的日聚合结果，不再读取请求级明细。
ORDER BY (tenant_id, user_id, usage_date, provider, model, api_key_name);

CREATE MATERIALIZED VIEW IF NOT EXISTS gateway_request_usage_daily_mv
TO gateway_request_usage_daily
AS
SELECT
    usage_date,
    assumeNotNull(tenant_id) AS tenant_id,
    assumeNotNull(user_id) AS user_id,
    provider,
    ifNull(model, '未记录') AS model,
    ifNull(api_key_name, '未记录') AS api_key_name,
    sum(input_tokens) AS input_tokens,
    sum(cached_input_tokens) AS cached_input_tokens,
    sum(output_tokens) AS output_tokens,
    sum(reasoning_output_tokens) AS reasoning_output_tokens,
    sum(total_tokens) AS total_tokens,
    count() AS request_count,
    countIf(status = 'success') AS success_count,
    countIf(status = 'abnormal') AS abnormal_count,
    countIf(status = 'failed') AS failed_count
FROM gateway_request_logs
-- 与现有 Dashboard 统计语义一致：鉴权前失败等未归属请求只保留明细，不进入用户用量。
WHERE tenant_id IS NOT NULL AND user_id IS NOT NULL
GROUP BY
    usage_date,
    tenant_id,
    user_id,
    provider,
    model,
    api_key_name;
