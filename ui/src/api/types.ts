export interface RegistrationInput { name: string; repo_path: string; store_path: string; model_path: string; daemon_path: string }
export interface Registration { id: string; config: RegistrationInput }
export interface ApiError { code: string; message: string; retryable: boolean }
export interface RequestEvent { repository_id: string; instance_id: string; sequence: number; completed_at_unix_ms: number; operation: string; elapsed_micros: number; outcome: string; error_code: string | null; result_count: number | null }
export interface ActivityPage { items: RequestEvent[]; next_cursor: string | null }
export interface Gap { repository_id: string; from_unix_ms: number; to_unix_ms: number; reason: string }
export interface ResourceSample { repository_id: string; resources: Record<string, unknown> }
export interface MetricBucket { from_unix_ms: number; to_unix_ms: number; request_count: number; sample_count: number; coverage_seconds: number; rate_per_second: number | null; p50_micros: number | null; p95_micros: number | null; resources: ResourceSample[] }
export interface MetricsResponse { buckets: MetricBucket[]; coverage_seconds: number; gap_markers: Gap[]; sample_count: number }
