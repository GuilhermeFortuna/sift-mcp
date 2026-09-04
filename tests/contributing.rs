//! CONTRIBUTING.md must point at `./ci.sh` rather than redefining the suite.

use std::fs;
use std::path::PathBuf;

#[test]
fn contributing_does_not_redefine_the_validation_command() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CONTRIBUTING.md");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("CONTRIBUTING.md must exist at {}: {e}", path.display()));

    assert!(
        text.contains("./ci.sh"),
        "CONTRIBUTING.md must name ./ci.sh as the validation command"
    );
    assert!(
        !text.contains("cargo fmt --all -- --check"),
        "CONTRIBUTING.md must not redefine the validation suite; that sequence lives in ci.sh"
    );
}
