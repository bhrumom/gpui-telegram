use crate::error::CliError;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct CliOutcome {
    pub stdout: String,
    pub exit_code: i32,
}

impl CliOutcome {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            exit_code: 0,
        }
    }

    pub fn with_exit_code(stdout: impl Into<String>, exit_code: i32) -> Self {
        Self {
            stdout: stdout.into(),
            exit_code,
        }
    }
}

pub fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn ok_envelope(data: Value) -> Value {
    json!({
        "success": true,
        "data": data,
        "error": Value::Null,
        "timestamp": now_timestamp(),
    })
}

pub fn err_envelope(code: &str, message: &str) -> Value {
    json!({
        "success": false,
        "data": Value::Null,
        "error": {
            "code": code,
            "message": message,
        },
        "timestamp": now_timestamp(),
    })
}

pub fn format_json(val: &Value, pretty: bool) -> Result<String, CliError> {
    if pretty {
        serde_json::to_string_pretty(val)
    } else {
        serde_json::to_string(val)
    }
    .map_err(|e| CliError::Execution(format!("Failed to serialize JSON output: {e}")))
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut chars = needle.chars();
    let Some(mut cur) = chars.next() else {
        return true;
    };
    for ch in haystack.chars() {
        if ch == cur {
            if let Some(next_ch) = chars.next() {
                cur = next_ch;
            } else {
                return true;
            }
        }
    }
    false
}

fn score_node_match(node: &Value, query_lower: &str) -> Option<u32> {
    let node_id = node
        .get("node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let display_name = node
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let device_id = node
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let host = node
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ip_address = node
        .get("ip_address")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let os_name = node
        .get("os_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let base_score = if node_id == query_lower
        || (!device_id.is_empty() && device_id == query_lower)
        || display_name == query_lower
    {
        400
    } else if display_name.starts_with(query_lower)
        || node_id.starts_with(query_lower)
        || (!device_id.is_empty() && device_id.starts_with(query_lower))
        || (!host.is_empty() && host.starts_with(query_lower))
    {
        300
    } else if display_name.contains(query_lower) {
        220
    } else if node_id.contains(query_lower)
        || (!device_id.is_empty() && device_id.contains(query_lower))
        || (!host.is_empty() && host.contains(query_lower))
        || ip_address.contains(query_lower)
        || os_name.contains(query_lower)
    {
        200
    } else if is_subsequence(query_lower, &display_name) || is_subsequence(query_lower, &node_id) {
        100
    } else {
        return None;
    };

    let online_bonus = if node.get("online").and_then(|v| v.as_bool()).unwrap_or(false) {
        10
    } else {
        0
    };
    let current_bonus = if node
        .get("is_current_device")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        1
    } else {
        0
    };
    Some(base_score + online_bonus + current_bonus)
}

pub fn fuzzy_match_node(nodes: &[Value], pattern: &str) -> Result<Value, CliError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(CliError::Validation(
            "Fuzzy match pattern '--m' cannot be empty".into(),
        ));
    }
    let query_lower = trimmed.to_ascii_lowercase();

    let mut scored: Vec<(u32, &Value)> = nodes
        .iter()
        .filter_map(|node| score_node_match(node, &query_lower).map(|score| (score, node)))
        .collect();

    if scored.is_empty() {
        return Err(CliError::Validation(format!(
            "No device matched pattern '--m {trimmed}'"
        )));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let top_score = scored[0].0;
    let tied: Vec<&Value> = scored
        .iter()
        .take_while(|(score, _)| *score == top_score)
        .map(|(_, node)| *node)
        .collect();

    if tied.len() > 1 {
        let labels: Vec<String> = tied
            .iter()
            .map(|node| {
                let name = node
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let id = node
                    .get("node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                format!("{name} ({id})")
            })
            .collect();
        return Err(CliError::Validation(format!(
            "Ambiguous pattern '--m {trimmed}' matched multiple devices: {}. Use a more specific pattern or exact node_id.",
            labels.join(", ")
        )));
    }

    Ok(tied[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_node_ranking_and_ambiguity() {
        let nodes = vec![
            json!({
                "node_id": "local:1:mbp",
                "display_name": "MacBook Pro",
                "online": true,
                "is_current_device": true
            }),
            json!({
                "node_id": "cloud:1:ubuntu",
                "display_name": "Prod Ubuntu Box",
                "host": "10.0.0.5",
                "online": true,
                "is_current_device": false
            }),
            json!({
                "node_id": "local:2:mini",
                "display_name": "Mac Mini",
                "online": false,
                "is_current_device": false
            }),
        ];

        // Subsequence match: mbp -> MacBook Pro
        let m1 = fuzzy_match_node(&nodes, "mbp").unwrap();
        assert_eq!(m1["node_id"], "local:1:mbp");

        // Substring match: ubuntu -> Prod Ubuntu Box
        let m2 = fuzzy_match_node(&nodes, "ubuntu").unwrap();
        assert_eq!(m2["node_id"], "cloud:1:ubuntu");

        // Online preference when both match prefix "mac"
        let m3 = fuzzy_match_node(&nodes, "mac").unwrap();
        assert_eq!(m3["node_id"], "local:1:mbp");

        // Empty or no match errors cleanly
        assert!(fuzzy_match_node(&nodes, "").is_err());
        assert!(fuzzy_match_node(&nodes, "windows-missing").is_err());
    }
}
