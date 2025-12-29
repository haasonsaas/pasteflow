use crate::detect::ContentType;
use crate::transforms::TransformKind;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub transform: Option<TransformKind>,
    #[serde(default)]
    pub llm: Option<LlmRule>,
    #[serde(default)]
    pub auto_accept: bool,
    #[serde(rename = "match", default)]
    pub matchers: Matchers,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Matchers {
    #[serde(default)]
    pub content_types: Option<Vec<ContentType>>,
    #[serde(default)]
    pub apps: Option<Vec<String>>,
    #[serde(default)]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRule {
    pub provider: String,
    pub model: String,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct MatchContext {
    pub text: String,
    pub content_types: Vec<ContentType>,
    pub active_app: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub rule: Rule,
    pub score: i32,
}

impl Rule {
    pub fn transform_kind(&self) -> Option<TransformKind> {
        self.transform
    }

    pub fn matches(&self, ctx: &MatchContext) -> Option<i32> {
        let mut score = 0;
        let mut specificity = 0;
        if let Some(content_types) = &self.matchers.content_types {
            let matched = content_types
                .iter()
                .filter(|t| ctx.content_types.contains(t))
                .count();
            if matched == 0 {
                return None;
            }
            score += 60 + (matched as i32 * 5);
            specificity += 1;
        }
        if let Some(apps) = &self.matchers.apps {
            let active = ctx.active_app.as_deref().unwrap_or("");
            let active_lower = active.to_lowercase();
            let mut matched = false;
            let mut exact = false;
            for app in apps {
                let needle = app.to_lowercase();
                if active_lower.contains(&needle) {
                    matched = true;
                    if active_lower == needle {
                        exact = true;
                    }
                }
            }
            if !matched {
                return None;
            }
            score += 50;
            if exact {
                score += 10;
            }
            specificity += 1;
        }
        if let Some(pattern) = &self.matchers.regex {
            let re = Regex::new(pattern).ok()?;
            if !re.is_match(&ctx.text) {
                return None;
            }
            score += 40;
            specificity += 1;
        }
        if specificity == 0 {
            score = 1;
        } else {
            score += specificity * 5;
        }
        if self.pinned {
            score += 1000;
        }
        Some(score)
    }
}

pub fn suggest_rules(rules: &[Rule], ctx: &MatchContext, max: usize) -> Vec<Suggestion> {
    let mut suggestions: Vec<Suggestion> = rules
        .iter()
        .filter_map(|rule| {
            rule.matches(ctx).map(|score| Suggestion {
                rule: rule.clone(),
                score,
            })
        })
        .collect();

    suggestions.sort_by(|a, b| b.score.cmp(&a.score));
    suggestions.truncate(max);
    suggestions
}
