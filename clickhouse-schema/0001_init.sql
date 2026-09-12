-- ClickHouse schema for request logs (created automatically on start).

CREATE TABLE IF NOT EXISTS request_logs
(
    id            UUID,
    request_id    String,
    timestamp     DateTime64(3, 'UTC'),
    virtual_key_id Nullable(UUID),
    key_name      LowCardinality(String),
    provider      LowCardinality(String),
    provider_name LowCardinality(String),
    model         LowCardinality(String),
    upstream_model LowCardinality(String),
    endpoint      LowCardinality(String),
    status        UInt16,
    error_message String DEFAULT '',
    error_type    LowCardinality(String) DEFAULT '',
    is_stream     Bool,
    prompt_tokens UInt64 DEFAULT 0,
    completion_tokens UInt64 DEFAULT 0,
    total_tokens  UInt64 DEFAULT 0,
    cost_usd      Float64 DEFAULT 0,
    duration_ms   UInt64 DEFAULT 0,
    first_byte_ms UInt64 DEFAULT 0,
    request_body  String DEFAULT '',
    response_body String DEFAULT ''
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(timestamp)
ORDER BY (timestamp, key_name, provider, model)
TTL toDateTime(timestamp) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;
