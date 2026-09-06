export interface RegistrationInput { name: string; repo_path: string; store_path: string; model_path: string; daemon_path: string }
export interface Registration { id: string; config: RegistrationInput }
export interface ApiError { code: string; message: string; retryable: boolean }
export type Lifecycle = 'starting' | 'ready' | 'indexing' | 'stale' | 'shutting_down';
export interface Freshness { head: string | null; indexed_commit: string | null; dirty: boolean | null; unavailable_reason: string | null; inspected_at_unix_ms: number }
export interface IndexProgress { phase: string; done: number; total: number | null; connection_id?: number; request_id?: number }
export interface IndexReport { commit: string; files_seen: number; files_indexed: number; files_excluded: number; files_unsupported: number; files_unparsed: number; chunks_added: number; chunks_reused: number; chunks_removed: number; embeddings_computed: number; chunks_truncated: number; parse_millis: number; embed_millis: number; store_millis: number; wall_millis: number; live_before: number; live_after: number }
export type JobState = 'queued' | 'running' | 'succeeded' | 'failed' | 'interrupted';
export type OperationType = 'start_daemon' | 'update_index' | 'full_rebuild';
export type OperationPhase = 'queued' | 'spawning_daemon' | 'waiting_for_socket' | 'connecting' | 'initializing_cuda' | 'loading_model' | 'opening_store' | 'ready' | 'walking' | 'parsing' | 'embedding' | 'storing' | 'compacting' | 'completed' | 'failed' | 'interrupted';
export interface OperationEvent { at_unix_ms: number; state: JobState; phase: OperationPhase; message: string }
export interface IndexJob { id: string; repository_id: string; state: JobState; operation_type: OperationType; phase: OperationPhase; progress: string | null; done: number; total: number | null; report: IndexReport | null; error_code: string | null; error_message: string | null; daemon_instance_id: string | null; started_at_unix_ms: number; updated_at_unix_ms: number; completed_at_unix_ms: number | null; events: OperationEvent[] }
export interface LastIndex { completed_at_unix_ms: number; outcome: string; error_code: string | null; report: IndexReport | null }
export interface DaemonStatus { lifecycle: Lifecycle; instance_id: string; observed_at_unix_ms: number; model_id: string | null; chunks_live: number | null; chunks_dead: number | null; indexed_commit: string | null; idle_seconds: number; uptime_seconds: number; current_progress: IndexProgress | null; last_index: LastIndex | null; resources: { sampled_at_unix_ms: number; execution_provider: 'cuda' | 'cpu' | null; device_id: string | null; device_name: string | null; device_utilization_percent: number | null; device_used_bytes: number | null; device_total_bytes: number | null; process_used_bytes: number | null; process_cpu_percent: number | null; model_used_bytes: number | null } }
export interface RepositoryStatus { status: DaemonStatus | null; collected_at_unix_ms: number; stale: boolean; connection_state: 'connected' | 'stopped' | 'unknown'; error_code: string | null }
export interface RequestEvent { repository_id: string; instance_id: string; sequence: number; completed_at_unix_ms: number; operation: string; elapsed_micros: number; outcome: string; error_code: string | null; result_count: number | null }
export interface ActivityPage { items: RequestEvent[]; next_cursor: string | null }
export interface Gap { repository_id: string; from_unix_ms: number; to_unix_ms: number; reason: string }
export interface ResourceSample { repository_id: string; resources: Record<string, unknown> }
export interface MetricBucket { from_unix_ms: number; to_unix_ms: number; request_count: number; sample_count: number; coverage_seconds: number; rate_per_second: number | null; p50_micros: number | null; p95_micros: number | null; resources: ResourceSample[] }
export interface MetricsResponse { buckets: MetricBucket[]; coverage_seconds: number; gap_markers: Gap[]; sample_count: number }
