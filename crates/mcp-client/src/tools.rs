//! Tool description artifact loaded from `descriptions.toml`.

use std::sync::LazyLock;

/// Bumped whenever any description text changes, so a behaviour change in an
/// agent can be attributed to a revision. Asserted by test to match the file.
pub const DESCRIPTIONS_VERSION: u32 = 1;

const RAW_TOML: &str = include_str!("../descriptions.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescription {
    pub name: &'static str,
    pub summary: &'static str,
    pub prefer_over: &'static str,
    pub examples: &'static [&'static str],
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_examples(examples: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = examples.into_iter().map(leak_str).collect();
    Box::leak(leaked.into_boxed_slice())
}

fn load_descriptions() -> &'static [ToolDescription] {
    let table: toml::Table = RAW_TOML.parse().expect("descriptions.toml must parse");
    let mut out = Vec::new();
    for (key, value) in &table {
        if key == "version" {
            continue;
        }
        let tool = value
            .as_table()
            .unwrap_or_else(|| panic!("descriptions.toml: {key} must be a table"));
        let summary = tool
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("descriptions.toml: {key}.summary"))
            .to_owned();
        let prefer_over = tool
            .get("prefer_over")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("descriptions.toml: {key}.prefer_over"))
            .to_owned();
        let examples: Vec<String> = tool
            .get("examples")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("descriptions.toml: {key}.examples"))
            .iter()
            .map(|v| {
                v.as_str()
                    .unwrap_or_else(|| panic!("descriptions.toml: {key}.examples entry"))
                    .to_owned()
            })
            .collect();
        out.push(ToolDescription {
            name: leak_str(key.clone()),
            summary: leak_str(summary),
            prefer_over: leak_str(prefer_over),
            examples: leak_examples(examples),
        });
    }
    out.sort_by(|a, b| a.name.cmp(b.name));
    Box::leak(out.into_boxed_slice())
}

static DESCRIPTIONS: LazyLock<&'static [ToolDescription]> = LazyLock::new(load_descriptions);

pub fn descriptions() -> &'static [ToolDescription] {
    *DESCRIPTIONS
}

/// summary + prefer_over + examples, rendered into the MCP description field.
pub fn rendered(name: &str) -> String {
    let Some(d) = descriptions().iter().find(|d| d.name == name) else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(d.summary);
    out.push_str("\n\n");
    out.push_str(d.prefer_over);
    out.push_str("\n\nExamples:\n");
    for ex in d.examples {
        out.push_str("- ");
        out.push_str(ex);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_toml() -> toml::Table {
        RAW_TOML
            .parse::<toml::Table>()
            .expect("descriptions.toml parses")
    }

    #[test]
    fn descriptions_toml_parses_and_covers_four_tools() {
        let table = parse_toml();
        for name in [
            "search_code",
            "find_similar_code",
            "get_symbol",
            "index_repository",
        ] {
            assert!(table.get(name).is_some(), "missing tool {name}");
        }
        assert_eq!(descriptions().len(), 4);
    }

    #[test]
    fn every_tool_has_prefer_over_and_example() {
        let table = parse_toml();
        for (key, value) in &table {
            if key == "version" {
                continue;
            }
            let tool = value.as_table().expect("tool table");
            let prefer = tool
                .get("prefer_over")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert!(
                !prefer.is_empty(),
                "tool {key} missing non-empty prefer_over"
            );
            let examples = tool
                .get("examples")
                .and_then(|v| v.as_array())
                .expect("examples array");
            assert!(
                !examples.is_empty(),
                "tool {key} needs at least one example"
            );
        }
        for d in descriptions() {
            assert!(!d.prefer_over.is_empty(), "{} prefer_over", d.name);
            assert!(!d.examples.is_empty(), "{} examples", d.name);
        }
    }

    #[test]
    fn descriptions_version_matches_toml() {
        let table = parse_toml();
        let version = table
            .get("version")
            .and_then(|v| v.as_integer())
            .expect("version") as u32;
        assert_eq!(version, DESCRIPTIONS_VERSION);
    }

    #[test]
    fn rendered_contains_summary_prefer_over_and_example() {
        for d in descriptions() {
            let text = rendered(d.name);
            assert!(text.contains(d.summary), "{} summary", d.name);
            assert!(text.contains(d.prefer_over), "{} prefer_over", d.name);
            assert!(text.contains(d.examples[0]), "{} example", d.name);
        }
    }
}
