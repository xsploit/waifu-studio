//! Keep copyable wire examples compatible with the actual protocol validators.
use waifu_studio::extension_protocol::{parse_action_line, parse_host_line};

#[test]
fn author_guide_jsonl_examples_are_valid_protocol_records() {
    let guide = include_str!("../docs/extension-authoring.md");
    let mut in_jsonl = false;
    let mut hosts = 0;
    let mut actions = 0;
    for (index, line) in guide.lines().enumerate() {
        if line == "```jsonl" {
            assert!(!in_jsonl);
            in_jsonl = true;
        } else if in_jsonl && line == "```" {
            in_jsonl = false;
        } else if in_jsonl && !line.trim().is_empty() {
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("guide line {}: {error}", index + 1));
            match value["type"].as_str() {
                Some("host") => {
                    parse_host_line(line.as_bytes())
                        .unwrap_or_else(|error| panic!("guide line {}: {error}", index + 1));
                    hosts += 1;
                }
                Some("action") => {
                    parse_action_line(line.as_bytes())
                        .unwrap_or_else(|error| panic!("guide line {}: {error}", index + 1));
                    actions += 1;
                }
                kind => panic!("guide line {}: unexpected envelope {kind:?}", index + 1),
            }
        }
    }
    assert!(!in_jsonl, "unterminated JSONL block");
    assert!(
        hosts >= 6,
        "retain handshake, target, reply, ACK, terminal and error examples"
    );
    assert!(actions >= 1, "retain a copyable action example");
}
