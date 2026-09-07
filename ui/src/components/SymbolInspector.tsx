import { useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { ApiError, SearchResult, SymbolResponse } from '../api/types';
import { generateLineNumbers, mapSymbolError, type MappedSymbolError } from '../search-lab';

export interface SymbolInspectorProps {
  repositoryId: string;
  result: SearchResult;
  onClear?: () => void;
  onSelectCandidate?: (candidate: string) => void;
  onRetrySearch?: () => void;
}

export function SymbolInspector({
  repositoryId,
  result,
  onClear,
  onSelectCandidate,
  onRetrySearch,
}: SymbolInspectorProps) {
  const [symbolData, setSymbolData] = useState<SymbolResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [errorState, setErrorState] = useState<MappedSymbolError | null>(null);
  const [copied, setCopied] = useState(false);
  const requestGeneration = useRef(0);
  const abortControllerRef = useRef<AbortController | null>(null);

  const fetchSymbol = (symbolName: string) => {
    const currentGeneration = ++requestGeneration.current;
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
    }
    const controller = new AbortController();
    abortControllerRef.current = controller;

    setLoading(true);
    setErrorState(null);
    setSymbolData(null);
    setCopied(false);

    api<SymbolResponse>(`/repositories/${encodeURIComponent(repositoryId)}/symbol`, {
      method: 'POST',
      body: JSON.stringify({
        file: result.file,
        symbol: symbolName,
      }),
      signal: controller.signal,
    })
      .then((data) => {
        if (currentGeneration !== requestGeneration.current) return;
        setSymbolData(data);
      })
      .catch((err: unknown) => {
        if (currentGeneration !== requestGeneration.current) return;
        const apiError = err as ApiError;
        setErrorState(mapSymbolError(apiError));
      })
      .finally(() => {
        if (currentGeneration === requestGeneration.current) {
          setLoading(false);
        }
      });
  };

  useEffect(() => {
    fetchSymbol(result.symbol);
    return () => {
      if (abortControllerRef.current) {
        abortControllerRef.current.abort();
      }
    };
  }, [repositoryId, result.file, result.symbol]);

  const handleCopy = async () => {
    if (!symbolData) return;
    try {
      await navigator.clipboard.writeText(symbolData.body);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback if clipboard API unavailable
    }
  };

  const lineNumbers = symbolData
    ? generateLineNumbers(symbolData.lines[0], symbolData.lines[1])
    : [];

  return (
    <aside className="panel symbol-inspector" aria-label="Symbol Inspector">
      <div className="symbol-inspector-header">
        <div className="symbol-meta">
          <p className="eyebrow">Symbol Inspector</p>
          <h2 className="result-symbol">{result.symbol}</h2>
          <span className="path">
            {result.file}:{result.lines[0]}–{result.lines[1]}
          </span>
        </div>
        <div className="symbol-actions">
          {symbolData && (
            <button
              className="button secondary"
              onClick={handleCopy}
              aria-label="Copy symbol source code"
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          )}
          {onClear && (
            <button
              className="button ghost"
              onClick={onClear}
              aria-label="Close symbol inspector"
            >
              Close
            </button>
          )}
        </div>
      </div>

      {loading && (
        <div className="panel loading-panel" role="status">
          Loading symbol definition…
        </div>
      )}

      {errorState && (
        <div className="alert danger" role="alert">
          <strong>{errorState.kind === 'not_found' ? 'Symbol Not Found' : errorState.kind === 'ambiguous' ? 'Ambiguous Symbol' : 'Error'}</strong>
          <p>{errorState.message}</p>

          {errorState.kind === 'not_found' && (
            <div style={{ marginTop: '10px' }}>
              <p className="muted">
                The symbol could not be found in the current index. The index may have changed since this search was run.
              </p>
              {onRetrySearch && (
                <button className="button primary" onClick={onRetrySearch}>
                  Run new search
                </button>
              )}
            </div>
          )}

          {errorState.kind === 'ambiguous' && errorState.candidates.length > 0 && (
            <div style={{ marginTop: '10px' }}>
              <p className="field-hint">Select a qualified candidate to inspect:</p>
              <ul className="candidate-list">
                {errorState.candidates.map((cand) => (
                  <li key={cand}>
                    <button
                      className="candidate-button"
                      onClick={() => {
                        if (onSelectCandidate) {
                          onSelectCandidate(cand);
                        } else {
                          fetchSymbol(cand);
                        }
                      }}
                    >
                      {cand}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {!loading && !errorState && symbolData && (
        <>
          {symbolData.signature && (
            <code className="result-signature" style={{ display: 'block', padding: '8px 12px' }}>
              {symbolData.signature}
            </code>
          )}

          <div className="source-view-container">
            <div className="source-line-numbers" aria-hidden="true">
              {lineNumbers.map((num) => (
                <div key={num}>{num}</div>
              ))}
            </div>
            <pre className="source-content">
              <code>{symbolData.body}</code>
            </pre>
          </div>
        </>
      )}
    </aside>
  );
}
