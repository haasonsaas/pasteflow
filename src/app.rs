use crate::config;
use crate::detect;
use crate::diff;
use crate::rules::{MatchContext, Rule, Suggestion};
use crate::transforms::TransformKind;
use arboard::Clipboard;
use enigo::{Enigo, Key, KeyboardControllable};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::error::Error;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowBuilder;
use wry::http::Request;
use wry::{WebView, WebViewBuilder};

struct PanelState {
    input: String,
    output: String,
    diff: String,
    active_app: Option<String>,
    content_types: Vec<crate::detect::ContentType>,
    active_app_key: String,
    search_query: Option<String>,
}

struct AppState {
    cfg: config::Config,
    clipboard: Clipboard,
    suggestions: Vec<Suggestion>,
    selected_rule_id: Option<String>,
    panel: PanelState,
    config_text: Option<String>,
    config_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcMessage {
    Paste,
    Copy,
    Cancel,
    SelectRule { id: String },
    ToggleAutoAccept { id: String, value: bool },
    TogglePinned { id: String, value: bool },
    UpdateSearch { value: String },
    RequestConfig,
    SaveConfig { raw: String },
}

#[derive(Debug, Serialize)]
struct UiRule {
    id: String,
    name: String,
    auto_accept: bool,
    uses_remote: bool,
    pinned: bool,
    detail: String,
    match_hint: String,
}

#[derive(Debug, Serialize)]
struct UiState {
    before: String,
    after: String,
    diff: String,
    suggestions: Vec<UiRule>,
    all_rules: Vec<UiRule>,
    selected_rule_id: Option<String>,
    active_app: Option<String>,
    content_types: Vec<String>,
    search_query: Option<String>,
    config_text: Option<String>,
    config_error: Option<String>,
}

struct TrayHandle {
    _tray: TrayIcon,
    show_id: MenuId,
    quit_id: MenuId,
}

#[derive(Debug)]
enum UserEvent {
    Ipc(IpcMessage),
    Menu(MenuEvent),
    Hotkey,
}

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run() -> AppResult<()> {
    let cfg = config::load_or_init()?;
    let clipboard = Clipboard::new().expect("clipboard available");

    let mut state = AppState {
        cfg,
        clipboard,
        suggestions: Vec::new(),
        selected_rule_id: None,
        panel: PanelState {
            input: String::new(),
            output: String::new(),
            diff: String::new(),
            active_app: None,
            content_types: Vec::new(),
            active_app_key: "global".to_string(),
            search_query: None,
        },
        config_text: None,
        config_error: None,
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()?;
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Pasteflow")
        .with_visible(false)
        .with_inner_size(LogicalSize::new(900.0, 640.0))
        .build(&event_loop)?;

    let html = include_str!("../assets/panel.html");
    let webview = WebViewBuilder::new(&window)
        .with_html(html)
        .with_ipc_handler(move |req: Request<String>| {
            if let Ok(event) = serde_json::from_str::<IpcMessage>(req.body()) {
                let _ = proxy.send_event(UserEvent::Ipc(event));
            }
        })
        .build()?;

    let tray = build_tray()?;

    let hotkey_manager = GlobalHotKeyManager::new()?;
    let mut registered = 0;
    for combo in hotkey_combos(&state.cfg) {
        if let Some(hotkey) = parse_hotkey(&combo) {
            if hotkey_manager.register(hotkey).is_ok() {
                registered += 1;
            }
        }
    }
    if registered == 0 {
        return Err("no valid hotkeys registered".into());
    }

    let hotkey_proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        let rx = GlobalHotKeyEvent::receiver();
        while rx.recv().is_ok() {
            let _ = hotkey_proxy.send_event(UserEvent::Hotkey);
        }
    });

    let menu_proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        let rx = MenuEvent::receiver();
        while let Ok(event) = rx.recv() {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }
    });

    event_loop.run(move |event, elwt| {

        match event {
            Event::UserEvent(UserEvent::Ipc(msg)) => {
                handle_ipc(&mut state, msg, &window, &webview);
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                if event.id == tray.show_id {
                    open_panel(&mut state, &window, &webview);
                } else if event.id == tray.quit_id {
                    elwt.exit();
                }
            }
            Event::UserEvent(UserEvent::Hotkey) => {
                open_panel(&mut state, &window, &webview);
            }
            Event::WindowEvent { event, .. } => {
                if matches!(event, WindowEvent::CloseRequested) {
                    window.set_visible(false);
                }
            }
            Event::AboutToWait => {
                elwt.set_control_flow(ControlFlow::Wait);
            }
            _ => {}
        }
    })?;

    #[allow(unreachable_code)]
    Ok(())
}

fn build_tray() -> AppResult<TrayHandle> {
    let menu = Menu::new();
    let show_item = MenuItem::new("Show Pasteflow", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();
    menu.append(&show_item).map_err(boxed)?;
    menu.append(&quit_item).map_err(boxed)?;

    let icon = load_icon();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Pasteflow")
        .with_icon(icon)
        .build()
        .map_err(boxed)?;

    Ok(TrayHandle {
        _tray: tray,
        show_id,
        quit_id,
    })
}

fn boxed<E: Error + Send + Sync + 'static>(err: E) -> Box<dyn Error + Send + Sync> {
    Box::new(err)
}

fn load_icon() -> tray_icon::Icon {
    let bytes = include_bytes!("../assets/icon.png");
    let image = image::load_from_memory(bytes).expect("icon decode");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).expect("icon rgba")
}

fn open_panel(state: &mut AppState, window: &winit::window::Window, webview: &WebView) {
    let text = state.clipboard.get_text().unwrap_or_default();
    let content_types = detect::detect_content_types(&text);
    let active_app = active_app_name();
    let app_key = active_app.clone().unwrap_or_else(|| "global".to_string());
    let ctx = MatchContext {
        text: text.clone(),
        content_types: content_types.clone(),
        active_app: active_app.clone(),
    };
    state.suggestions =
        crate::rules::suggest_rules(&state.cfg.rules, &ctx, state.cfg.ui.suggestions);
    state.selected_rule_id = state
        .cfg
        .ui_state
        .get(&app_key)
        .and_then(|prefs| prefs.selected_rule_id.clone());
    if state.selected_rule_id.is_none() {
        state.selected_rule_id = state
            .suggestions
            .first()
            .map(|suggestion| suggestion.rule.id.clone());
    }
    state.panel.input = text;
    state.panel.active_app = active_app;
    state.panel.content_types = content_types;
    state.panel.active_app_key = app_key.clone();
    state.panel.search_query = state
        .cfg
        .ui_state
        .get(&app_key)
        .and_then(|prefs| prefs.search.clone());
    if let Some(selected) = &state.selected_rule_id {
        if state.cfg.rules.iter().all(|rule| &rule.id != selected) {
            state.selected_rule_id = state
                .suggestions
                .first()
                .map(|suggestion| suggestion.rule.id.clone());
            update_ui_prefs(state, None, state.selected_rule_id.clone());
        }
    }
    refresh_preview(state);

    if let Some(rule) = selected_rule(state) {
        if rule.auto_accept {
            apply_paste(state);
            return;
        }
    }

    send_state(state, webview);
    window.set_visible(true);
    let _ = window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
    let _ = window.focus_window();
}

fn handle_ipc(state: &mut AppState, msg: IpcMessage, window: &winit::window::Window, webview: &WebView) {
    match msg {
        IpcMessage::Paste => {
            apply_paste(state);
            window.set_visible(false);
        }
        IpcMessage::Copy => {
            apply_copy(state);
            window.set_visible(false);
        }
        IpcMessage::Cancel => {
            window.set_visible(false);
        }
        IpcMessage::SelectRule { id } => {
            state.selected_rule_id = Some(id);
            update_ui_prefs(state, None, state.selected_rule_id.clone());
            refresh_preview(state);
            send_state(state, webview);
        }
        IpcMessage::ToggleAutoAccept { id, value } => {
            if let Some(rule) = state.cfg.rules.iter_mut().find(|rule| rule.id == id) {
                rule.auto_accept = value;
                let _ = config::save(&state.cfg);
            }
            refresh_preview(state);
            send_state(state, webview);
        }
        IpcMessage::TogglePinned { id, value } => {
            if let Some(rule) = state.cfg.rules.iter_mut().find(|rule| rule.id == id) {
                rule.pinned = value;
                let _ = config::save(&state.cfg);
            }
            rebuild_suggestions(state);
            refresh_preview(state);
            send_state(state, webview);
        }
        IpcMessage::UpdateSearch { value } => {
            update_ui_prefs(state, Some(value), None);
            send_state(state, webview);
        }
        IpcMessage::RequestConfig => {
            match config::load_raw() {
                Ok(raw) => {
                    state.config_text = Some(raw);
                    state.config_error = None;
                }
                Err(err) => {
                    state.config_error = Some(err.to_string());
                }
            }
            send_state(state, webview);
        }
        IpcMessage::SaveConfig { raw } => {
            match config::parse_raw(&raw) {
                Ok(cfg) => {
                    if let Err(err) = config::write_raw(&raw) {
                        state.config_error = Some(err.to_string());
                    } else {
                        state.cfg = cfg;
                        state.config_text = Some(raw);
                        state.config_error = None;
                        rebuild_suggestions(state);
                        refresh_preview(state);
                    }
                }
                Err(err) => {
                    state.config_error = Some(err.to_string());
                }
            }
            send_state(state, webview);
        }
    }
}

fn refresh_preview(state: &mut AppState) {
    let input = state.panel.input.clone();
    let output = if let Some(rule) = selected_rule(state) {
        apply_rule(rule, &input)
    } else {
        input.clone()
    };
    state.panel.output = output.clone();
    state.panel.diff = diff::unified_diff(&input, &output);
}

fn rebuild_suggestions(state: &mut AppState) {
    let text = state.panel.input.clone();
    let content_types = detect::detect_content_types(&text);
    let ctx = MatchContext {
        text,
        content_types,
        active_app: state.panel.active_app.clone().or_else(active_app_name),
    };
    state.suggestions =
        crate::rules::suggest_rules(&state.cfg.rules, &ctx, state.cfg.ui.suggestions);
    if let Some(prefs) = state.cfg.ui_state.get(&state.panel.active_app_key) {
        state.panel.search_query = prefs.search.clone();
    }
    if let Some(id) = &state.selected_rule_id {
        if state.cfg.rules.iter().all(|rule| &rule.id != id) {
            state.selected_rule_id = state
                .suggestions
                .first()
                .map(|suggestion| suggestion.rule.id.clone());
        }
    }
}

fn apply_rule(rule: &Rule, input: &str) -> String {
    if let Some(kind) = rule.transform_kind() {
        match kind.apply(input) {
            Ok(out) => out,
            Err(err) => format!("Transform error: {}", err),
        }
    } else if rule.llm.is_some() {
        "LLM rule is configured but not enabled in this MVP.".to_string()
    } else {
        input.to_string()
    }
}

fn selected_rule(state: &AppState) -> Option<&Rule> {
    let id = state.selected_rule_id.as_deref()?;
    if let Some(rule) = state
        .suggestions
        .iter()
        .find(|suggestion| suggestion.rule.id == id)
        .map(|suggestion| &suggestion.rule)
    {
        return Some(rule);
    }
    state.cfg.rules.iter().find(|rule| rule.id == id)
}

fn send_state(state: &AppState, webview: &WebView) {
    let suggestions: Vec<UiRule> = state
        .suggestions
        .iter()
        .map(|suggestion| ui_rule(&suggestion.rule))
        .collect();

    let all_rules: Vec<UiRule> = state.cfg.rules.iter().map(ui_rule).collect();

    let content_types = state
        .panel
        .content_types
        .iter()
        .map(content_type_label)
        .collect();

    let ui_state = UiState {
        before: state.panel.input.clone(),
        after: state.panel.output.clone(),
        diff: state.panel.diff.clone(),
        suggestions,
        all_rules,
        selected_rule_id: state.selected_rule_id.clone(),
        active_app: state.panel.active_app.clone(),
        content_types,
        search_query: state.panel.search_query.clone(),
        config_text: state.config_text.clone(),
        config_error: state.config_error.clone(),
    };

    if let Ok(payload) = serde_json::to_string(&ui_state) {
        let script = format!("window.__SET_STATE__({});", payload);
        let _ = webview.evaluate_script(&script);
    }
}

fn apply_copy(state: &mut AppState) {
    let _ = state.clipboard.set_text(state.panel.output.clone());
}

fn apply_paste(state: &mut AppState) {
    apply_copy(state);
    let mut enigo = Enigo::new();
    enigo.key_down(Key::Meta);
    enigo.key_click(Key::Layout('v'));
    enigo.key_up(Key::Meta);
}

fn content_type_label(content_type: &crate::detect::ContentType) -> String {
    match content_type {
        crate::detect::ContentType::Json => "json".to_string(),
        crate::detect::ContentType::Yaml => "yaml".to_string(),
        crate::detect::ContentType::Text => "text".to_string(),
        crate::detect::ContentType::List => "list".to_string(),
        crate::detect::ContentType::Timestamp => "timestamp".to_string(),
    }
}

fn ui_rule(rule: &Rule) -> UiRule {
    UiRule {
        id: rule.id.clone(),
        name: rule.name.clone(),
        auto_accept: rule.auto_accept,
        uses_remote: rule.llm.is_some(),
        pinned: rule.pinned,
        detail: rule_detail(rule),
        match_hint: rule_match_hint(rule),
    }
}

fn rule_detail(rule: &Rule) -> String {
    let mut base = if let Some(kind) = rule.transform_kind() {
        format!("Transform: {}", transform_label(kind))
    } else if let Some(llm) = &rule.llm {
        format!("LLM: {}/{}", llm.provider, llm.model)
    } else {
        "Transform: none".to_string()
    };
    if let Some(desc) = &rule.description {
        if !desc.trim().is_empty() {
            base.push_str(" - ");
            base.push_str(desc.trim());
        }
    }
    base
}

fn rule_match_hint(rule: &Rule) -> String {
    let mut parts = Vec::new();
    if let Some(types) = &rule.matchers.content_types {
        let list = types
            .iter()
            .map(content_type_label)
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("types: {}", list));
    }
    if let Some(apps) = &rule.matchers.apps {
        let list = apps.join(", ");
        parts.push(format!("apps: {}", list));
    }
    if let Some(regex) = &rule.matchers.regex {
        let trimmed = if regex.len() > 60 {
            format!("{}...", &regex[..60])
        } else {
            regex.clone()
        };
        parts.push(format!("regex: {}", trimmed));
    }
    if parts.is_empty() {
        "Match: any".to_string()
    } else {
        format!("Match: {}", parts.join(" | "))
    }
}

fn transform_label(kind: TransformKind) -> &'static str {
    match kind {
        TransformKind::JsonPrettify => "json_prettify",
        TransformKind::JsonMinify => "json_minify",
        TransformKind::JsonToYaml => "json_to_yaml",
        TransformKind::YamlToJson => "yaml_to_json",
        TransformKind::StripFormatting => "strip_formatting",
        TransformKind::BulletNormalize => "bullet_normalize",
        TransformKind::TimestampNormalize => "timestamp_normalize",
    }
}

fn update_ui_prefs(state: &mut AppState, search: Option<String>, selected: Option<String>) {
    let key = state.panel.active_app_key.clone();
    let entry = state.cfg.ui_state.entry(key).or_default();
    if let Some(value) = search {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            entry.search = None;
            state.panel.search_query = None;
        } else {
            entry.search = Some(trimmed.clone());
            state.panel.search_query = Some(trimmed);
        }
    }
    if let Some(id) = selected {
        entry.selected_rule_id = Some(id);
    }
    let _ = config::save(&state.cfg);
}

fn active_app_name() -> Option<String> {
    match active_win_pos_rs::get_active_window() {
        Ok(window) => Some(window.app_name),
        Err(_) => None,
    }
}

fn hotkey_combos(cfg: &config::Config) -> Vec<String> {
    let mut combos = Vec::new();
    combos.push(cfg.hotkey.combo.clone());
    for combo in cfg.hotkey.apps.values() {
        combos.push(combo.clone());
    }
    let mut seen = HashSet::new();
    combos
        .into_iter()
        .filter(|combo| seen.insert(combo.to_lowercase()))
        .collect()
}

fn parse_hotkey(combo: &str) -> Option<HotKey> {
    let parts: Vec<&str> = combo.split('+').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut modifiers = Modifiers::empty();
    let mut code: Option<Code> = None;

    for part in parts {
        match part.to_lowercase().as_str() {
            "cmd" | "command" | "meta" => modifiers |= Modifiers::META,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            key => {
                code = Some(match key {
                    "v" => Code::KeyV,
                    "c" => Code::KeyC,
                    "p" => Code::KeyP,
                    _ => return None,
                });
            }
        }
    }

    Some(HotKey::new(Some(modifiers), code?))
}
