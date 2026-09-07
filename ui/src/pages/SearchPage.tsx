import { useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { Registration, SearchResponse, SearchResult } from '../api/types';
import { SearchResults } from '../components/SearchResults';
import { SymbolInspector } from '../components/SymbolInspector';
import {
  DEFAULT_LIMIT,
  getSearchEndpoint,
  getSearchPayload,
  shouldSubmitOnKeyDown,
  validateLimit,
  type SearchMode,
} from '../search-lab';

export function SearchPage() {
  const [repositories, setRepositories] = useState<Registration[]>([]);
  const [selectedRepoId, setSelectedRepoId] = useState<string>('');
  const [mode, setMode] = useState<SearchMode>('query');
  const [inputVal, setInputVal] = useState<string>('');
  const [limitVal, setLimitVal] = useState<string>(String(DEFAULT_LIMIT));
  const [limitError, setLimitError] = useState<string | null>(null);

  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [searchResponse, setSearchResponse] = useState<SearchResponse | null>(null);
  const [selectedResult, setSelectedResult] = useState<SearchResult | null>(null);

  const searchGeneration = useRef(0);
  const searchAbortRef = useRef<AbortController | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Initialize repository list and initial repository from URL search param
  useEffect(() => {
    api<Registration[]>('/repositories')
      .then((repos) => {
        setRepositories(repos);
        const params = new URLSearchParams(window.location.search);
        const urlRepo = params.get('repository');
        if (urlRepo && repos.some((r) => r.id === urlRepo)) {
          setSelectedRepoId(urlRepo);
        } else if (repos.length > 0) {
          setSelectedRepoId(repos[0].id);
        }
      })
      .catch((e: { message?: string }) => {
        setSearchError(e.message ?? 'Failed to load repositories.');
      });
  }, []);

  // When repository changes, update URL param (only repo identity), reset results and selection, DO NOT submit automatically
  const handleRepositoryChange = (newRepoId: string) => {
    if (newRepoId === selectedRepoId) return;

    if (searchAbortRef.current) {
      searchAbortRef.current.abort();
    }
    searchGeneration.current++;

    setSelectedRepoId(newRepoId);
    setSearchResponse(null);
    setSelectedResult(null);
    setSearchError(null);

    const url = new URL(window.location.href);
    if (newRepoId) {
      url.searchParams.set('repository', newRepoId);
    } else {
      url.searchParams.delete('repository');
    }
    window.history.replaceState(null, '', url.toString());
  };

  const handleClear = () => {
    if (searchAbortRef.current) {
      searchAbortRef.current.abort();
    }
    searchGeneration.current++;

    setInputVal('');
    setSearchResponse(null);
    setSelectedResult(null);
    setSearchError(null);
    textareaRef.current?.focus();
  };

  const handleSubmit = async () => {
    if (!selectedRepoId || !inputVal.trim()) {
      return;
    }

    const limitValidation = validateLimit(limitVal);
    if (!limitValidation.valid || limitValidation.value === undefined) {
      setLimitError(limitValidation.error ?? 'Invalid limit');
      return;
    }
    setLimitError(null);

    if (searchAbortRef.current) {
      searchAbortRef.current.abort();
    }
    const controller = new AbortController();
    searchAbortRef.current = controller;

    const currentGeneration = ++searchGeneration.current;
    setLoading(true);
    setSearchError(null);
    setSelectedResult(null);

    const endpoint = getSearchEndpoint(selectedRepoId, mode);
    const payload = getSearchPayload(mode, inputVal.trim(), limitValidation.value);

    try {
      const response = await api<SearchResponse>(endpoint, {
        method: 'POST',
        body: JSON.stringify(payload),
        signal: controller.signal,
      });

      if (currentGeneration === searchGeneration.current) {
        setSearchResponse(response);
      }
    } catch (e: unknown) {
      if (currentGeneration === searchGeneration.current) {
        const err = e as { message?: string };
        setSearchError(err.message ?? 'Search request failed.');
      }
    } finally {
      if (currentGeneration === searchGeneration.current) {
        setLoading(false);
      }
    }
  };

  return (
    <>
      <header className="page-header">
        <div>
          <p className="eyebrow">Retrieval Lab</p>
          <h1>Search Lab</h1>
          <p className="lede">
            Execute natural-language code queries and snippet similarity searches with direct symbol inspection.
          </p>
        </div>
      </header>

      {repositories.length === 0 ? (
        <div className="panel empty-panel">
          <span className="empty-glyph">＋</span>
          <h2>No repositories registered</h2>
          <p className="muted">Attach a repository first to run queries and inspect symbols.</p>
          <a className="button primary" href="/repositories/new">
            Attach repository
          </a>
        </div>
      ) : (
        <>
          <div className="panel search-input-panel">
            <div className="field" style={{ marginBottom: '16px' }}>
              <label htmlFor="repo-select">Target Repository</label>
              <select
                id="repo-select"
                className="limit-input"
                style={{ width: '100%', maxWidth: '360px' }}
                value={selectedRepoId}
                onChange={(e) => handleRepositoryChange(e.target.value)}
              >
                {repositories.map((repo) => (
                  <option key={repo.id} value={repo.id}>
                    {repo.config.name} ({repo.config.repo_path})
                  </option>
                ))}
              </select>
            </div>

            <div className="search-tabs" role="tablist">
              <button
                role="tab"
                aria-selected={mode === 'query'}
                className={`tab-button ${mode === 'query' ? 'active' : ''}`}
                onClick={() => setMode('query')}
              >
                Natural Language Query
              </button>
              <button
                role="tab"
                aria-selected={mode === 'similar'}
                className={`tab-button ${mode === 'similar' ? 'active' : ''}`}
                onClick={() => setMode('similar')}
              >
                Similar Code Snippet
              </button>
            </div>

            <div className="field">
              <label htmlFor="search-input">
                {mode === 'query' ? 'Query Text' : 'Pasted Code Snippet'}
              </label>
              <textarea
                id="search-input"
                ref={textareaRef}
                className="search-textarea"
                rows={4}
                placeholder={
                  mode === 'query'
                    ? 'e.g., normalize decoder timestamp or error: index lock'
                    : 'Paste a snippet to find analogous implementations...'
                }
                value={inputVal}
                onChange={(e) => setInputVal(e.target.value)}
                onKeyDown={(e) => {
                  if (shouldSubmitOnKeyDown(e)) {
                    e.preventDefault();
                    void handleSubmit();
                  }
                }}
              />
              <span className="field-hint">Press Ctrl+Enter or Cmd+Enter to search.</span>
            </div>

            <div className="search-controls">
              <div className="limit-input-group">
                <label htmlFor="limit-input">Result limit (1–20):</label>
                <input
                  id="limit-input"
                  type="number"
                  min={1}
                  max={20}
                  className="limit-input"
                  value={limitVal}
                  onChange={(e) => {
                    setLimitVal(e.target.value);
                    const v = validateLimit(e.target.value);
                    setLimitError(v.valid ? null : (v.error ?? null));
                  }}
                />
                {limitError && <span className="field-error">{limitError}</span>}
              </div>

              <div className="search-buttons">
                <button
                  type="button"
                  className="button secondary"
                  onClick={handleClear}
                  disabled={loading && !inputVal}
                >
                  Clear
                </button>
                <button
                  type="button"
                  className="button primary"
                  disabled={loading || !inputVal.trim() || Boolean(limitError)}
                  onClick={() => void handleSubmit()}
                >
                  {loading ? 'Searching…' : 'Search'}
                </button>
              </div>
            </div>
          </div>

          {searchError && (
            <div className="alert danger" role="alert">
              {searchError}
            </div>
          )}

          {searchResponse && (
            <div className="search-layout">
              <div className="search-results-pane">
                <SearchResults
                  response={searchResponse}
                  selectedResult={selectedResult}
                  onSelect={(res) => setSelectedResult(res)}
                />
              </div>

              {selectedResult && (
                <div className="symbol-inspector-pane">
                  <SymbolInspector
                    repositoryId={selectedRepoId}
                    result={selectedResult}
                    onClear={() => setSelectedResult(null)}
                    onRetrySearch={() => void handleSubmit()}
                  />
                </div>
              )}
            </div>
          )}
        </>
      )}
    </>
  );
}
