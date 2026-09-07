import type { SearchResponse, SearchResult } from '../api/types';
import { formatScore, formatTiming, hasDegradation } from '../search-lab';

export interface SearchResultsProps {
  response: SearchResponse;
  onSelect: (result: SearchResult) => void;
  selectedResult?: SearchResult | null;
}

export function SearchResults({ response, onSelect, selectedResult }: SearchResultsProps) {
  const { results, diagnostics } = response;
  const isDegraded = hasDegradation(diagnostics);

  return (
    <div className="search-results-container">
      <div className="search-diagnostics panel">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Search Diagnostics</p>
            <h2>Performance & Timings</h2>
          </div>
          {isDegraded ? (
            <span className="status-badge danger">Degraded</span>
          ) : (
            <span className="status-badge success">Optimal</span>
          )}
        </div>

        {isDegraded && (
          <div className="alert danger" role="alert">
            <strong>Degraded Retriever State:</strong>{' '}
            {!diagnostics.lexical_ok && (
              <span>Lexical search failed{diagnostics.lexical_error ? `: ${diagnostics.lexical_error}` : ''}. </span>
            )}
            {!diagnostics.dense_ok && (
              <span>Dense retrieval failed{diagnostics.dense_error ? `: ${diagnostics.dense_error}` : ''}.</span>
            )}
          </div>
        )}

        <div className="stage-timings-grid">
          <div className="timing-stat">
            <span>Embed</span>
            <strong>{formatTiming(diagnostics.stage_millis.embed)}</strong>
          </div>
          <div className="timing-stat">
            <span>Lexical</span>
            <strong>{formatTiming(diagnostics.stage_millis.lexical)}</strong>
          </div>
          <div className="timing-stat">
            <span>Dense</span>
            <strong>{formatTiming(diagnostics.stage_millis.dense)}</strong>
          </div>
          <div className="timing-stat">
            <span>Fuse</span>
            <strong>{formatTiming(diagnostics.stage_millis.fuse)}</strong>
          </div>
          <div className="timing-stat">
            <span>Assemble</span>
            <strong>{formatTiming(diagnostics.stage_millis.assemble)}</strong>
          </div>
          <div className="timing-stat highlight">
            <span>Total</span>
            <strong>{formatTiming(diagnostics.stage_millis.total)}</strong>
          </div>
        </div>
      </div>

      <div className="results-list" role="feed" aria-label="Search results">
        {results.length === 0 ? (
          <div className="panel empty-panel">
            <span className="empty-glyph">∅</span>
            <h2>No results found</h2>
            <p className="muted">Try refining your search query or selecting a different repository.</p>
          </div>
        ) : (
          results.map((result, index) => {
            const isSelected =
              selectedResult &&
              selectedResult.file === result.file &&
              selectedResult.symbol === result.symbol &&
              selectedResult.lines[0] === result.lines[0] &&
              selectedResult.lines[1] === result.lines[1];

            return (
              <article
                key={`${result.file}-${result.symbol}-${result.lines[0]}-${index}`}
                className={`panel result-card ${isSelected ? 'selected' : ''}`}
                tabIndex={0}
                role="button"
                aria-pressed={isSelected ? true : false}
                onClick={() => onSelect(result)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    onSelect(result);
                  }
                }}
              >
                <div className="result-card-header">
                  <div className="result-title-group">
                    <span className="card-kicker">#{index + 1}</span>
                    <h3 className="result-symbol">{result.symbol}</h3>
                    <code className="result-signature">{result.signature}</code>
                  </div>
                  <div className="result-location">
                    <span className="path">{result.file}</span>
                    <span className="line-range">
                      L{result.lines[0]}–L{result.lines[1]}
                    </span>
                  </div>
                </div>

                {result.doc && <p className="result-doc">{result.doc}</p>}

                <div className="result-preview">
                  <pre>
                    <code>{result.preview}</code>
                  </pre>
                </div>

                <div className="result-scores">
                  <div className="score-item">
                    <span>Fused score</span>
                    <strong>{formatScore(result.fused_score)}</strong>
                  </div>
                  <div className="score-item">
                    <span>Dense score</span>
                    <strong>{formatScore(result.dense_score)}</strong>
                  </div>
                  <div className="score-item">
                    <span>Lexical score</span>
                    <strong>{formatScore(result.lexical_score)}</strong>
                  </div>
                </div>
              </article>
            );
          })
        )}
      </div>
    </div>
  );
}
