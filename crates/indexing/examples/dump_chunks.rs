//! Walk a repository, apply exclusions, and print symbol chunks as JSON.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use indexing::{Chunker, Exclusions, HEAD_SNIFF_BYTES, Language, MAX_FILE_BYTES, SkipReason};
use serde::Serialize;

#[derive(Serialize)]
struct ChunkOut<'a> {
    file: &'a str,
    language: &'a str,
    symbol: &'a str,
    symbol_type: &'a str,
    signature: &'a str,
    doc_first_line: Option<&'a str>,
    line_start: u32,
    line_end: u32,
    fragment: Option<u32>,
    content_hash: String,
    body: &'a str,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1);
    let mut timing = false;
    let mut repo: Option<PathBuf> = None;
    for a in args {
        if a == "--timing" {
            timing = true;
        } else if repo.is_none() {
            repo = Some(PathBuf::from(a));
        }
    }
    let repo = repo.ok_or("usage: dump_chunks <repo-path> [--timing]")?;
    let exclusions = Exclusions::for_repository(&repo)?;
    let mut chunker = Chunker::new()?;

    let mut files_seen = 0u64;
    let mut files_chunked = 0u64;
    let mut files_skipped = 0u64;
    let mut files_unsupported = 0u64;
    let mut files_unparsed = 0u64;
    let mut total_chunks = 0u64;

    let start = Instant::now();
    let mut stack = vec![repo.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if exclusions.check_path(&path).is_some() {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            files_seen += 1;
            if let Some(reason) = exclusions.check_path(&path) {
                files_skipped += 1;
                if matches!(reason, SkipReason::UnsupportedLanguage) {
                    files_unsupported += 1;
                }
                continue;
            }
            let Some(language) = Language::from_path(&path) else {
                files_unsupported += 1;
                files_skipped += 1;
                continue;
            };

            let meta = fs::metadata(&path)?;
            if exclusions.check_size(meta.len()).is_some() || meta.len() > MAX_FILE_BYTES {
                files_skipped += 1;
                continue;
            }

            let bytes = fs::read(&path)?;
            let head_len = HEAD_SNIFF_BYTES.min(bytes.len());
            if exclusions.check_head(&bytes[..head_len]).is_some() {
                files_skipped += 1;
                continue;
            }
            let Ok(source) = std::str::from_utf8(&bytes) else {
                files_skipped += 1;
                continue;
            };

            let rel = path
                .strip_prefix(&repo)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let file = chunker.chunk_file(&rel, language, source);
            if !file.diagnostics.is_empty() && file.chunks.is_empty() {
                files_unparsed += 1;
                continue;
            }
            files_chunked += 1;
            total_chunks += file.chunks.len() as u64;

            let stdout = io::stdout();
            let mut out = stdout.lock();
            for c in &file.chunks {
                let row = ChunkOut {
                    file: &c.record.file,
                    language: &c.record.language,
                    symbol: &c.record.symbol,
                    symbol_type: &c.record.symbol_type,
                    signature: &c.record.signature,
                    doc_first_line: c.record.doc_first_line.as_deref(),
                    line_start: c.record.line_start,
                    line_end: c.record.line_end,
                    fragment: c.fragment,
                    content_hash: hex(c.record.content_hash.as_bytes()),
                    body: &c.body,
                };
                serde_json::to_writer(&mut out, &row)?;
                out.write_all(b"\n")?;
            }
        }
    }

    if timing {
        let elapsed = start.elapsed().as_secs_f64().max(1e-9);
        eprintln!("files_seen={files_seen}");
        eprintln!("files_chunked={files_chunked}");
        eprintln!("files_skipped={files_skipped}");
        eprintln!("files_unsupported={files_unsupported}");
        eprintln!("files_unparsed={files_unparsed}");
        eprintln!("total_chunks={total_chunks}");
        eprintln!("files_per_sec={:.2}", files_chunked as f64 / elapsed);
        eprintln!("chunks_per_sec={:.2}", total_chunks as f64 / elapsed);
        eprintln!("elapsed_secs={elapsed:.4}");
    }

    Ok(())
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
