use chrono::{DateTime, NaiveDate, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// Pre-compiled regexes for performance
static BULLET_LIST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*([-*•])\s+\S+").unwrap());
static RELATIVE_NOW_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^now([+-])(\d+)([smhd])$").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Json,
    Yaml,
    Text,
    List,
    Timestamp,
}

pub fn detect_content_types(input: &str) -> Vec<ContentType> {
    let mut types = vec![ContentType::Text];
    let trimmed = input.trim();

    let is_json = is_json(trimmed);
    if is_json {
        types.push(ContentType::Json);
    } else if is_yaml(trimmed) {
        types.push(ContentType::Yaml);
    }

    if is_bullet_list(trimmed) {
        types.push(ContentType::List);
    }

    if is_timestamp(trimmed) {
        types.push(ContentType::Timestamp);
    }

    types
}

fn is_json(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(input).is_ok()
}

fn is_yaml(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    // YAML parser accepts almost anything as valid YAML (plain scalars)
    // Require structural elements to avoid false positives on plain text
    let has_structure = input.contains(':')
        || input.starts_with('-')
        || input.contains("\n-")
        || input.contains('[')
        || input.contains('{');
    if !has_structure {
        return false;
    }
    // Parse and ensure it's not just a plain scalar
    match serde_yaml::from_str::<serde_yaml::Value>(input) {
        Ok(value) => !matches!(
            value,
            serde_yaml::Value::String(_)
                | serde_yaml::Value::Number(_)
                | serde_yaml::Value::Bool(_)
        ),
        Err(_) => false,
    }
}

fn is_bullet_list(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    let lines: Vec<&str> = input.lines().collect();
    if lines.len() < 2 {
        return false;
    }
    let bullet_lines = lines
        .iter()
        .filter(|line| BULLET_LIST_RE.is_match(line))
        .count();
    bullet_lines >= 2
}

fn is_timestamp(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }

    if input == "now" || input.starts_with("now+") || input.starts_with("now-") {
        return true;
    }

    if input.chars().all(|c| c.is_ascii_digit()) {
        let len = input.len();
        return len == 10 || len == 13;
    }

    if DateTime::parse_from_rfc3339(input).is_ok() {
        return true;
    }

    if NaiveDate::parse_from_str(input, "%Y-%m-%d").is_ok() {
        return true;
    }

    false
}

pub fn normalize_timestamp(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed == "now" {
        return Some(Utc::now().to_rfc3339());
    }

    if let Some(relative) = parse_relative_now(trimmed) {
        return Some(relative.to_rfc3339());
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        let value: i64 = trimmed.parse().ok()?;
        if trimmed.len() == 13 {
            let dt = DateTime::<Utc>::from_timestamp_millis(value)?;
            return Some(dt.to_rfc3339());
        }
        let dt = DateTime::<Utc>::from_timestamp(value, 0)?;
        return Some(dt.to_rfc3339());
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.timestamp().to_string());
    }

    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0)?;
        let dt = DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc);
        return Some(dt.timestamp().to_string());
    }

    None
}

fn parse_relative_now(input: &str) -> Option<DateTime<Utc>> {
    let caps = RELATIVE_NOW_RE.captures(input)?;
    let sign = caps.get(1)?.as_str();
    let amount: i64 = caps.get(2)?.as_str().parse().ok()?;
    let unit = caps.get(3)?.as_str();

    let seconds = match unit {
        "s" => amount,
        "m" => amount * 60,
        "h" => amount * 3600,
        "d" => amount * 86400,
        _ => return None,
    };

    let now = Utc::now();
    if sign == "-" {
        Some(now - chrono::Duration::seconds(seconds))
    } else {
        Some(now + chrono::Duration::seconds(seconds))
    }
}
