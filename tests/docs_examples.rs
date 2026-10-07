//! Every request example in the documentation parses as the tools read it, so a
//! documented spelling never drifts from the parser.

use serde_json::Value;
use ultra_edit::{Change, EditRequest};

const DOCS: [(&str, &str); 7] = [
    ("README.md", include_str!("../README.md")),
    ("docs/reference.md", include_str!("../docs/reference.md")),
    (
        "SKILL.md",
        include_str!("../plugin/claude-code/skills/edit/SKILL.md"),
    ),
    (
        "contract.md",
        include_str!("../plugin/claude-code/skills/edit/references/contract.md"),
    ),
    (
        "recovery.md",
        include_str!("../plugin/claude-code/skills/edit/references/recovery.md"),
    ),
    (
        "targets.md",
        include_str!("../plugin/claude-code/skills/edit/references/targets.md"),
    ),
    (
        "instructions.md",
        include_str!("../plugin/claude-code/instructions.md"),
    ),
];

/// The fenced JSON blocks of a Markdown text, and the card's unfenced example.
fn json_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```json\n") {
        let body = &rest[start + "```json\n".len()..];
        let end = body.find("```").expect("an unclosed json block");
        blocks.push(body[..end].to_owned());
        rest = &body[end..][3..];
    }
    if blocks.is_empty()
        && let Some(start) = text.find("{\"files\"")
    {
        let end = text[start..].find("]}]}").unwrap() + start + 4;
        blocks.push(text[start..end].to_owned());
    }
    blocks
}

#[test]
fn every_documented_request_parses() {
    let mut requests = 0;
    for (name, text) in DOCS {
        for block in json_blocks(text) {
            let value: Value = serde_json::from_str(&block)
                .unwrap_or_else(|error| panic!("{name}: {error}\n{block}"));
            if value.get("files").is_some() {
                serde_json::from_value::<EditRequest>(value)
                    .unwrap_or_else(|error| panic!("{name}: {error}\n{block}"));
                requests += 1;
            } else if let Some(changes) = value.get("changes") {
                serde_json::from_value::<Vec<Change>>(changes.clone())
                    .unwrap_or_else(|error| panic!("{name}: {error}\n{block}"));
                requests += 1;
            }
        }
    }
    // README, reference (3), SKILL, contract, recovery, and the card.
    assert_eq!(requests, 8);
}
