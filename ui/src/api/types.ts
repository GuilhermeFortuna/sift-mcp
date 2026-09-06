export interface RegistrationInput { name: string; repo_path: string; store_path: string; model_path: string; daemon_path: string }
export interface Registration { id: string; config: RegistrationInput }
export interface ApiError { code: string; message: string; retryable: boolean }
export interface RequestEvent { repository_id: string; instance_id: string; sequence: number; completed_at_unix_ms: number; operation: string; elapsed_micros: number; outcome: string; error_code: string | null; result_count: number | null }
export interface ActivityPage { items: RequestEvent[]; next_cursor: string | null }
export interface MetricsResponse { buckets: unknown[]; coverage_seconds: number; gap_markers: number[]; sample_count: number }
