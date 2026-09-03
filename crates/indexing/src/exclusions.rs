use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::ChunkError;

/// Soft cap on indexed file size. Files larger than this are refused.
pub const MAX_FILE_BYTES: u64 = 1_048_576;

/// How many leading bytes are sniffed for binary content.
pub const HEAD_SNIFF_BYTES: usize = 8192;

/// Why a path was skipped. Returned so a caller can explain any decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    SecretPattern(&'static str),
    VendorDirectory(&'static str),
    GeneratedPattern(&'static str),
    GitIgnored,
    TooLarge { bytes: u64, limit: u64 },
    BinaryContent,
    UnsupportedLanguage,
}

/// Compiled built-in globs plus repository ignore rules.
pub struct Exclusions {
    secret_globs: GlobSet,
    secret_patterns: Vec<&'static str>,
    vendor_globs: GlobSet,
    vendor_patterns: Vec<&'static str>,
    generated_globs: GlobSet,
    generated_patterns: Vec<&'static str>,
    gitignore: Option<Gitignore>,
    root: PathBuf,
}

impl Exclusions {
    pub fn for_repository(root: &Path) -> Result<Self, ChunkError> {
        let (secret_globs, secret_patterns) = build_globs(&[
            (".env", ".env"),
            (".env.*", ".env.*"),
            ("**/.env", ".env"),
            ("**/.env.*", ".env.*"),
            ("*.pem", "*.pem"),
            ("**/*.pem", "*.pem"),
            ("*.key", "*.key"),
            ("**/*.key", "*.key"),
            ("id_rsa*", "id_rsa*"),
            ("**/id_rsa*", "id_rsa*"),
            ("credentials.*", "credentials.*"),
            ("**/credentials.*", "credentials.*"),
            ("secrets.*", "secrets.*"),
            ("**/secrets.*", "secrets.*"),
        ])?;

        let (vendor_globs, vendor_patterns) = build_globs(&[
            ("node_modules", "node_modules/"),
            ("**/node_modules", "node_modules/"),
            ("**/node_modules/**", "node_modules/"),
            ("vendor", "vendor/"),
            ("**/vendor", "vendor/"),
            ("**/vendor/**", "vendor/"),
            ("target", "target/"),
            ("**/target", "target/"),
            ("**/target/**", "target/"),
            ("dist", "dist/"),
            ("**/dist", "dist/"),
            ("**/dist/**", "dist/"),
            (".venv", ".venv/"),
            ("**/.venv", ".venv/"),
            ("**/.venv/**", ".venv/"),
        ])?;

        let (generated_globs, generated_patterns) = build_globs(&[
            ("*_pb2.py", "*_pb2.py"),
            ("**/*_pb2.py", "*_pb2.py"),
            ("*.generated.*", "*.generated.*"),
            ("**/*.generated.*", "*.generated.*"),
        ])?;

        let gitignore = {
            let gi_path = root.join(".gitignore");
            if gi_path.is_file() {
                let mut builder = GitignoreBuilder::new(root);
                builder.add(&gi_path);
                Some(builder.build().map_err(|e| ChunkError::Ignore(e.to_string()))?)
            } else {
                None
            }
        };

        Ok(Self {
            secret_globs,
            secret_patterns,
            vendor_globs,
            vendor_patterns,
            generated_globs,
            generated_patterns,
            gitignore,
            root: root.to_path_buf(),
        })
    }

    /// Path-only decision. Made before the file is opened.
    pub fn check_path(&self, path: &Path) -> Option<SkipReason> {
        let rel = if path.is_absolute() {
            path.strip_prefix(&self.root).unwrap_or(path)
        } else {
            path
        };

        if let Some(idx) = self.secret_globs.matches(rel).into_iter().next() {
            return Some(SkipReason::SecretPattern(self.secret_patterns[idx]));
        }
        // Also match on file name alone for patterns like `.env`.
        if let Some(name) = rel.file_name()
            && let Some(idx) = self.secret_globs.matches(Path::new(name)).into_iter().next()
        {
            return Some(SkipReason::SecretPattern(self.secret_patterns[idx]));
        }

        // Vendor: any path component matching a vendor directory name.
        for component in rel.components() {
            let c = component.as_os_str();
            if c == "node_modules" {
                return Some(SkipReason::VendorDirectory("node_modules/"));
            }
            if c == "vendor" {
                return Some(SkipReason::VendorDirectory("vendor/"));
            }
            if c == "target" {
                return Some(SkipReason::VendorDirectory("target/"));
            }
            if c == "dist" {
                return Some(SkipReason::VendorDirectory("dist/"));
            }
            if c == ".venv" {
                return Some(SkipReason::VendorDirectory(".venv/"));
            }
        }
        if let Some(idx) = self.vendor_globs.matches(rel).into_iter().next() {
            return Some(SkipReason::VendorDirectory(self.vendor_patterns[idx]));
        }

        if let Some(idx) = self.generated_globs.matches(rel).into_iter().next() {
            return Some(SkipReason::GeneratedPattern(self.generated_patterns[idx]));
        }
        if let Some(name) = rel.file_name()
            && let Some(idx) = self
                .generated_globs
                .matches(Path::new(name))
                .into_iter()
                .next()
        {
            return Some(SkipReason::GeneratedPattern(self.generated_patterns[idx]));
        }

        if let Some(gi) = &self.gitignore {
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.root.join(path)
            };
            let is_dir = abs.is_dir()
                || abs
                    .to_string_lossy()
                    .ends_with('/');
            let matched = gi.matched_path_or_any_parents(&abs, is_dir);
            if matched.is_ignore() {
                return Some(SkipReason::GitIgnored);
            }
        }

        None
    }

    /// Size decision from metadata; does not read file contents.
    pub fn check_size(&self, bytes: u64) -> Option<SkipReason> {
        if bytes > MAX_FILE_BYTES {
            Some(SkipReason::TooLarge {
                bytes,
                limit: MAX_FILE_BYTES,
            })
        } else {
            None
        }
    }

    /// Content decision on the first bytes only, after `check_path` passes.
    pub fn check_head(&self, head: &[u8]) -> Option<SkipReason> {
        if memchr::memchr(0, head).is_some() {
            Some(SkipReason::BinaryContent)
        } else {
            None
        }
    }
}

fn build_globs(
    entries: &[(&str, &'static str)],
) -> Result<(GlobSet, Vec<&'static str>), ChunkError> {
    let mut builder = GlobSetBuilder::new();
    let mut patterns = Vec::with_capacity(entries.len());
    for (glob, label) in entries {
        builder
            .add(Glob::new(glob).map_err(|e| ChunkError::Glob(e.to_string()))?);
        patterns.push(*label);
    }
    let set = builder
        .build()
        .map_err(|e| ChunkError::Glob(e.to_string()))?;
    Ok((set, patterns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn exclusions_at(root: &Path) -> Exclusions {
        Exclusions::for_repository(root).expect("exclusions")
    }

    fn assert_secret(path: &str, pattern: &str) {
        let root = tempfile::tempdir().unwrap();
        let ex = exclusions_at(root.path());
        match ex.check_path(Path::new(path)) {
            Some(SkipReason::SecretPattern(p)) => assert_eq!(p, pattern),
            other => panic!("expected SecretPattern({pattern}), got {other:?} for {path}"),
        }
    }

    fn assert_vendor(path: &str, pattern: &str) {
        let root = tempfile::tempdir().unwrap();
        let ex = exclusions_at(root.path());
        match ex.check_path(Path::new(path)) {
            Some(SkipReason::VendorDirectory(p)) => assert_eq!(p, pattern),
            other => panic!("expected VendorDirectory({pattern}), got {other:?} for {path}"),
        }
    }

    fn assert_generated(path: &str, pattern: &str) {
        let root = tempfile::tempdir().unwrap();
        let ex = exclusions_at(root.path());
        match ex.check_path(Path::new(path)) {
            Some(SkipReason::GeneratedPattern(p)) => assert_eq!(p, pattern),
            other => panic!("expected GeneratedPattern({pattern}), got {other:?} for {path}"),
        }
    }

    #[test]
    fn excludes_dotenv() {
        assert_secret(".env", ".env");
    }

    #[test]
    fn excludes_dotenv_local() {
        assert_secret(".env.local", ".env.*");
    }

    #[test]
    fn excludes_pem() {
        assert_secret("certs/server.pem", "*.pem");
    }

    #[test]
    fn excludes_key() {
        assert_secret("tls/server.key", "*.key");
    }

    #[test]
    fn excludes_id_rsa() {
        assert_secret("id_rsa", "id_rsa*");
    }

    #[test]
    fn excludes_credentials_json() {
        assert_secret("credentials.json", "credentials.*");
    }

    #[test]
    fn excludes_secrets_yaml() {
        assert_secret("secrets.yaml", "secrets.*");
    }

    #[test]
    fn excludes_node_modules() {
        assert_vendor("node_modules/leftpad/index.js", "node_modules/");
    }

    #[test]
    fn excludes_vendor() {
        assert_vendor("vendor/pkg/mod.go", "vendor/");
    }

    #[test]
    fn excludes_target() {
        assert_vendor("target/debug/foo", "target/");
    }

    #[test]
    fn excludes_dist() {
        assert_vendor("dist/bundle.js", "dist/");
    }

    #[test]
    fn excludes_venv() {
        assert_vendor(".venv/lib/python.py", ".venv/");
    }

    #[test]
    fn excludes_pb2() {
        assert_generated("foo_pb2.py", "*_pb2.py");
    }

    #[test]
    fn excludes_generated_ts() {
        assert_generated("bar.generated.ts", "*.generated.*");
    }

    #[test]
    fn honours_gitignore_in_addition_to_builtins() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitignore"), "scratch/\n").unwrap();
        fs::create_dir_all(root.path().join("scratch")).unwrap();
        fs::write(root.path().join("scratch/tmp.rs"), "fn x() {}\n").unwrap();
        let ex = exclusions_at(root.path());

        assert_eq!(
            ex.check_path(&root.path().join("scratch/tmp.rs")),
            Some(SkipReason::GitIgnored)
        );
        // Built-in still applies even when not in .gitignore.
        assert_eq!(
            ex.check_path(Path::new("node_modules/x.js")),
            Some(SkipReason::VendorDirectory("node_modules/"))
        );
    }

    #[test]
    fn excluded_path_is_never_opened() {
        let root = tempfile::tempdir().unwrap();
        let nm = root.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        let sentinel = nm.join("secret.bin");
        fs::write(&sentinel, b"should-not-be-read").unwrap();
        let mut perms = fs::metadata(&sentinel).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&sentinel, perms).unwrap();

        let ex = exclusions_at(root.path());
        let reason = ex.check_path(&sentinel);
        assert_eq!(reason, Some(SkipReason::VendorDirectory("node_modules/")));
        // Reaching here without an I/O error is the point: check_path is path-only.
        let _ = reason;

        // Restore perms so tempfile cleanup succeeds.
        let mut perms = fs::metadata(&sentinel).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&sentinel, perms).unwrap();
    }

    #[test]
    fn null_bytes_are_binary() {
        let root = tempfile::tempdir().unwrap();
        let ex = exclusions_at(root.path());
        assert_eq!(
            ex.check_head(b"abc\0def"),
            Some(SkipReason::BinaryContent)
        );
    }

    #[test]
    fn oversized_file_is_rejected_regardless_of_extension() {
        let root = tempfile::tempdir().unwrap();
        let ex = exclusions_at(root.path());
        let bytes = MAX_FILE_BYTES + 1;
        match ex.check_size(bytes) {
            Some(SkipReason::TooLarge {
                bytes: b,
                limit: l,
            }) => {
                assert_eq!(b, bytes);
                assert_eq!(l, MAX_FILE_BYTES);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // A .rs extension does not override the size rule.
        assert!(ex.check_path(Path::new("big.rs")).is_none());
    }
}
