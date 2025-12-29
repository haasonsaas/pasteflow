# Pasteflow

[![CI](https://github.com/haasonsaas/pasteflow/actions/workflows/ci.yml/badge.svg)](https://github.com/haasonsaas/pasteflow/actions/workflows/ci.yml)

Rules-first paste transforms with a mandatory preview and diff. Think "prettier + jq + LLM" glued into `Cmd+Shift+V`, but deterministic by default.

## What it does (MVP)
- macOS menu bar app with a global hotkey (default: `Cmd+Shift+V`).
- Reads clipboard text, suggests the top rules, and shows a before/after diff.
- One explicit accept path: **Paste**, **Copy**, or **Cancel**.
- Deterministic transforms out of the box: JSON prettify/minify, JSON↔YAML, plain-text cleanup, bullet normalization, timestamp normalization.

## Why rules-first
Pasteflow always runs deterministic rules first. LLM rules are supported in the config format but are **off by default** and require explicit per-rule opt-in.

## Quick start
```bash
cargo run --release
```

The app runs in the menu bar. Use the hotkey to open the diff panel.

## Shortcuts
- `/` or `Cmd+K`: focus rule search.
- `↑ / ↓`: cycle suggested rules.
- `Cmd+Enter`: paste.
- `Esc`: close panel (or clear search, or close config editor).

## In-app config editor
Open **Edit config** to view and edit the TOML config in-app. Changes are validated before saving.
You can also edit rule descriptions, pinned flags, and per-app hotkeys directly in the panel.
Raw TOML edits show a live diff preview and inline validation, with a Revert button to discard changes.

## Rule info + sticky state
- Rule info panel shows transform, match hints, and flags for the selected rule.
- Rule chips show `P` (pinned), `A` (auto-accept), and `R` (remote model) badges.
- Search query and selected rule are remembered per active app.

## Pinned rules
Set `pinned = true` on any rule to keep it at the top of suggestions (when it matches), or toggle it in the Rule Info panel.

## Per-app hotkeys
You can register additional hotkeys per app. Changes apply immediately.

## Config editor upgrades
- Live TOML validation with diff preview + Revert.
- Hotkey conflict warnings are surfaced in the panel.
- Match-strength sorting when searching rules.
- Transform errors show as an inline banner instead of silently mutating output.

## Activity log
The Recent panel shows the last 5 Paste/Copy actions with the rule used and a snippet.

## Metrics
Pasteflow shows before/after character + line counts and diff add/remove counts in the header.

## Notes
- Pasteflow simulates `Cmd+V` after copying the transformed text; macOS may prompt for Accessibility permission.
- LLM rules are supported in config but are off by default and require explicit per-rule opt-in.

## Config
Pasteflow loads `~/.config/pasteflow/config.toml`. If it doesn't exist, the app will create it from the bundled defaults.

Default config: `config/default.toml`

Rule matching supports:
- `content_types`: `json`, `yaml`, `text`, `list`, `timestamp`
- `apps`: active app name (e.g. "Terminal", "Slack")
- `regex`: a regex that must match clipboard text

Example rule:
```toml
[[rules]]
id = "json_prettify"
name = "JSON Prettify"
description = "Pretty-print JSON with stable indentation."
pinned = true
transform = "json_prettify"
auto_accept = false
[rules.match]
content_types = ["json"]
apps = ["Terminal", "Visual Studio Code"]
```

Per-app hotkeys:
```toml
[hotkey]
combo = "Cmd+Shift+V"
apps = { "Slack" = "Cmd+Shift+P", "Terminal" = "Cmd+Shift+K" }
```

## Status
This is an early MVP. macOS-only for now. Windows parity is planned after v1 proves the model.
