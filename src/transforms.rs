use crate::detect::normalize_timestamp;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformKind {
    JsonPrettify,
    JsonMinify,
    JsonToYaml,
    YamlToJson,
    StripFormatting,
    BulletNormalize,
    TimestampNormalize,
}

#[derive(thiserror::Error, Debug)]
pub enum TransformError {
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unsupported timestamp format")]
    Timestamp,
}

impl TransformKind {
    pub fn apply(&self, input: &str) -> Result<String, TransformError> {
        match self {
            TransformKind::JsonPrettify => {
                let value: serde_json::Value = serde_json::from_str(input)?;
                Ok(serde_json::to_string_pretty(&value)?)
            }
            TransformKind::JsonMinify => {
                let value: serde_json::Value = serde_json::from_str(input)?;
                Ok(serde_json::to_string(&value)?)
            }
            TransformKind::JsonToYaml => {
                let value: serde_json::Value = serde_json::from_str(input)?;
                let mut yaml = serde_yaml::to_string(&value)?;
                if yaml.starts_with("---") {
                    yaml = yaml.trim_start_matches("---\n").to_string();
                }
                Ok(yaml)
            }
            TransformKind::YamlToJson => {
                let value: serde_json::Value = serde_yaml::from_str(input)?;
                Ok(serde_json::to_string_pretty(&value)?)
            }
            TransformKind::StripFormatting => Ok(normalize_whitespace(input)),
            TransformKind::BulletNormalize => Ok(normalize_bullets(input)),
            TransformKind::TimestampNormalize => {
                normalize_timestamp(input).ok_or(TransformError::Timestamp)
            }
        }
    }
}

fn normalize_whitespace(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();

    while matches!(lines.first(), Some(first) if first.trim().is_empty()) {
        lines.remove(0);
    }
    while matches!(lines.last(), Some(last) if last.trim().is_empty()) {
        lines.pop();
    }

    let mut out = lines.join("\n");
    let multi_blank = Regex::new(r"\n{3,}").expect("regex compiles");
    out = multi_blank.replace_all(&out, "\n\n").to_string();
    out
}

fn normalize_bullets(input: &str) -> String {
    let bullet_re = Regex::new(r"^(\s*)([-*•])\s+(.*)$").expect("regex compiles");
    let mut out = Vec::new();
    for line in input.replace("\r\n", "\n").lines() {
        if let Some(caps) = bullet_re.captures(line) {
            let indent = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let content = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            out.push(format!("{}- {}", indent, content.trim()));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::TransformKind;

    #[test]
    fn json_prettify_roundtrip() {
        let input = "{\"a\":1,\"b\":[2,3]}";
        let output = TransformKind::JsonPrettify.apply(input).unwrap();
        assert!(output.contains("\n"));
    }

    #[test]
    fn json_minify_roundtrip() {
        let input = "{\n  \"a\": 1\n}";
        let output = TransformKind::JsonMinify.apply(input).unwrap();
        assert_eq!(output, "{\"a\":1}");
    }

    #[test]
    fn bullet_normalize() {
        let input = "* One\n  • Two";
        let output = TransformKind::BulletNormalize.apply(input).unwrap();
        assert!(output.starts_with("- One"));
        assert!(output.contains("  - Two"));
    }
}

