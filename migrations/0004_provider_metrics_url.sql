-- Optionale Prometheus-/metrics-endpoint-url pro provider.
-- Wird fuer echte queue-tiefen (z.b. vLLM num_requests_waiting) im
-- live-dashboard abgefragt. NULL = keine metrics verfuegbar (N/A).
ALTER TABLE providers ADD COLUMN IF NOT EXISTS metrics_url TEXT;
