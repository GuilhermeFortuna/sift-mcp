//! Proxy efficiency: bytes read before the correct symbol enters context.

use std::path::Path;
use std::process::Command;

use retrieval::{FusionConfig, SearchResponse, Searcher};
use serde_json::json;

use crate::error::EvalError;
use crate::metrics::BytesBeforeHit;
use crate::mine::Label;
use crate::metrics::percentile;

/// Default keyword baseline an agent already has.
pub const BASELINE_COMMAND: &str = "rg --files-with-matches -F";

/// Bytes of serialized MCP-style results up to and including the first hit,
/// versus bytes of files a keyword search returns until the expected file.
pub fn bytes_before_hit(
    searcher: &Searcher<'_>,
    repo: &Path,
    label: &Label,
    config: &FusionConfig,
) -> Result<(u64, u64, String), EvalError> {
    let response = searcher
        .search(&label.query, 20, config)
        .map_err(|e| EvalError::Retrieval(e.to_string()))?;
    let mcp = mcp_bytes_to_first_hit(&response, &label.expected);

    let expected_file = label
        .expected
        .first()
        .map(|(f, _)| f.as_str())
        .unwrap_or("");
    let (baseline, cmd) = baseline_bytes_to_file(repo, &label.query, expected_file)?;
    Ok((mcp, baseline, cmd))
}

pub fn median_bytes_before_hit(
    searcher: &Searcher<'_>,
    repo: &Path,
    labels: &[Label],
    config: &FusionConfig,
) -> Result<BytesBeforeHit, EvalError> {
    let mut mcp_vals = Vec::new();
    let mut base_vals = Vec::new();
    let mut cmd = BASELINE_COMMAND.to_string();
    for label in labels {
        let (mcp, baseline, c) = bytes_before_hit(searcher, repo, label, config)?;
        mcp_vals.push(mcp as f64);
        base_vals.push(baseline as f64);
        cmd = c;
    }
    mcp_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    base_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(BytesBeforeHit {
        mcp_median: percentile(&mcp_vals, 0.50) as u64,
        baseline_median: percentile(&base_vals, 0.50) as u64,
        baseline_command: cmd,
    })
}

fn mcp_bytes_to_first_hit(
    response: &SearchResponse,
    expected: &[(String, String)],
) -> u64 {
    let mut total = 0u64;
    for result in &response.results {
        let serialized = json!({
            "file": result.file,
            "symbol": result.symbol,
            "signature": result.signature,
            "doc": result.doc,
            "preview": result.preview,
            "lines": result.lines,
        });
        total += serialized.to_string().len() as u64;
        if expected
            .iter()
            .any(|(f, s)| f == &result.file && s == &result.symbol)
        {
            return total;
        }
    }
    total
}

fn baseline_bytes_to_file(
    repo: &Path,
    query: &str,
    expected_file: &str,
) -> Result<(u64, String), EvalError> {
    // Record the command verbatim; use a stable keyword from the query.
    let keyword = query.split_whitespace().next().unwrap_or(query);
    let cmd = format!("{BASELINE_COMMAND} {keyword}");
    let output = Command::new("rg")
        .args(["--files-with-matches", "-F", keyword])
        .current_dir(repo)
        .output();

    let paths = match output {
        Ok(out) if out.status.success() || out.status.code() == Some(1) => {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }
        Ok(out) => {
            return Err(EvalError::message(format!(
                "rg failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Err(e) => {
            // Fallback: list all files in lexical path order for environments without rg.
            let _ = e;
            return fallback_walk_bytes(repo, expected_file, &cmd);
        }
    };

    let mut total = 0u64;
    for rel in paths {
        let full = repo.join(&rel);
        let bytes = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
        total += bytes;
        if rel.replace('\\', "/") == expected_file.replace('\\', "/") {
            return Ok((total, cmd));
        }
    }
    Ok((total, cmd))
}

fn fallback_walk_bytes(
    repo: &Path,
    expected_file: &str,
    cmd: &str,
) -> Result<(u64, String), EvalError> {
    let mut files = Vec::new();
    fn walk(dir: &Path, repo: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(&path, repo, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(repo, repo, &mut files)?;
    files.sort();
    let mut total = 0u64;
    for path in files {
        let rel = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if rel == expected_file.replace('\\', "/") {
            break;
        }
    }
    Ok((total, cmd.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrieval::{SearchDiagnostics, SearchResponse, SearchResult, StageTimings};

    #[test]
    fn proxy_kpi_hand_computed_bytes_and_records_baseline_command() {
        let response = SearchResponse {
            results: vec![
                SearchResult {
                    file: "other.rs".into(),
                    symbol: "nope".into(),
                    signature: "fn nope()".into(),
                    doc: None,
                    preview: "aaa".into(),
                    lines: [1, 2],
                    lexical_score: Some(1.0),
                    dense_score: None,
                    fused_score: 1.0,
                },
                SearchResult {
                    file: "hit.rs".into(),
                    symbol: "target".into(),
                    signature: "fn target()".into(),
                    doc: None,
                    preview: "bbb".into(),
                    lines: [3, 4],
                    lexical_score: Some(0.5),
                    dense_score: None,
                    fused_score: 0.5,
                },
            ],
            diagnostics: SearchDiagnostics {
                lexical_ok: true,
                dense_ok: true,
                lexical_error: None,
                dense_error: None,
                stage_millis: StageTimings {
                    embed: 0,
                    lexical: 0,
                    dense: 0,
                    fuse: 0,
                    assemble: 0,
                    total: 0,
                },
            },
        };
        let expected = vec![("hit.rs".into(), "target".into())];
        let mcp = mcp_bytes_to_first_hit(&response, &expected);
        let first = json!({
            "file": "other.rs",
            "symbol": "nope",
            "signature": "fn nope()",
            "doc": null,
            "preview": "aaa",
            "lines": [1, 2],
        })
        .to_string()
        .len() as u64;
        let second = json!({
            "file": "hit.rs",
            "symbol": "target",
            "signature": "fn target()",
            "doc": null,
            "preview": "bbb",
            "lines": [3, 4],
        })
        .to_string()
        .len() as u64;
        assert_eq!(mcp, first + second);

        let dir = tempfile::TempDir::new().unwrap();
        // Sorted walk: a.rs (4) then hit.rs (8) → 12 when expected is hit.rs.
        std::fs::write(dir.path().join("a.rs"), "aaaa").unwrap();
        std::fs::write(dir.path().join("hit.rs"), "hhhhhhhh").unwrap();
        let (bytes, cmd) = fallback_walk_bytes(dir.path(), "hit.rs", BASELINE_COMMAND).unwrap();
        assert_eq!(bytes, 12);
        assert_eq!(cmd, BASELINE_COMMAND);
        assert!(cmd.starts_with("rg "));
    }
}
