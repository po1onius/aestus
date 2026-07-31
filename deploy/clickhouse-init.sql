CREATE TABLE IF NOT EXISTS gateway_request_logs
(
    request_id UUID,
    provider LowCardinality(String),
    route LowCardinality(String),
    api_key_name Nullable(String),
    user_id Nullable(UUID),
    username Nullable(String),
    provider_group_id Nullable(UUID),
    provider_group_name Nullable(String),
    model Nullable(String),
    reasoning Nullable(String),
    service_tier Nullable(String),
    fast_mode Nullable(Bool),
    is_compaction Nullable(Bool),
    request_started_at DateTime64(3, 'UTC'),
    response_started_at Nullable(DateTime64(3, 'UTC')),
    response_finished_at Nullable(DateTime64(3, 'UTC')),
    input_tokens Int64,
    cached_input_tokens Int64,
    output_tokens Int64,
    reasoning_output_tokens Int64,
    total_tokens Int64,
    status LowCardinality(String),
    extra String
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(request_started_at)
-- 日志页按时间范围和请求 ID 做 keyset 分页，排序键优先放时间列，减少当天日志
-- 翻页时的扫描范围；provider/model 这类分析维度后续可以单独建物化视图或投影。
ORDER BY (request_started_at, request_id);
