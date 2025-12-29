use crate::rules::Rule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub hotkey: HotkeyConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub ui_state: HashMap<String, UiAppState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub combo: String,
    #[serde(default)]
    pub apps: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub suggestions: usize,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            combo: "Cmd+Shift+V".to_string(),
            apps: HashMap::new(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { suggestions: 3 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiAppState {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub selected_rule_id: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("failed to load config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
}

pub fn config_path() -> PathBuf {
    let mut root = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    root.push(".config");
    root.push("pasteflow");
    root.push("config.toml");
    root
}

pub fn load_or_init() -> Result<Config, ConfigError> {
    let path = config_path();
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&raw)?;
        return Ok(cfg);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let defaults = include_str!("../config/default.toml");
    fs::write(&path, defaults)?;
    let cfg: Config = toml::from_str(defaults)?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<(), ConfigError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(cfg).expect("config serialization should not fail");
    fs::write(path, raw)?;
    Ok(())
}

pub fn load_raw() -> Result<String, ConfigError> {
    let path = config_path();
    if !path.exists() {
        let _ = load_or_init()?;
    }
    Ok(fs::read_to_string(path)?)
}

pub fn parse_raw(raw: &str) -> Result<Config, ConfigError> {
    let cfg: Config = toml::from_str(raw)?;
    Ok(cfg)
}

pub fn write_raw(raw: &str) -> Result<(), ConfigError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, raw)?;
    Ok(())
}
