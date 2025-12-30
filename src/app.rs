use crate::config;
use crate::detect;
use crate::diff;
use crate::rules::{MatchContext, Rule, Suggestion};
use crate::transforms::TransformKind;
use arboard::Clipboard;
use chrono::{DateTime, Local, Utc};
use enigo::{Enigo, Key, KeyboardControllable};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use wry::http::Request;
use wry::{WebView, WebViewBuilder};

struct PanelState {
    input: String,
    output: String,
    diff: String,
    error: Option<String>,
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
    config_draft_error: Option<String>,
    config_diff: Option<String>,
    hotkey_manager: GlobalHotKeyManager,
    registered_hotkeys: Vec<HotKey>,
    hotkey_map: HashMap<u32, HotkeyRule>,
    hotkey_warnings: Vec<String>,
    history: Vec<HistoryItem>,
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
    UpdateRuleDescription { id: String, value: String },
    UpdateHotkeyCombo { combo: String },
    UpdateHotkeyApp { app: String, combo: String },
    RemoveHotkeyApp { app: String },
    UpdateSearch { value: String },
    RequestConfig,
    UpdateConfigDraft { raw: String },
    SaveConfig { raw: String },
}

#[derive(Debug, Serialize)]
struct UiRule {
    id: String,
    name: String,
    auto_accept: bool,
    uses_remote: bool,
    pinned: bool,
    score: i32,
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
    config: UiConfigState,
    config_text: Option<String>,
    config_error: Option<String>,
    config_draft_error: Option<String>,
    config_diff: Option<String>,
    history: Vec<UiHistoryItem>,
    stats: UiStats,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct UiConfigState {
    hotkey_combo: String,
    hotkey_apps: Vec<UiHotkeyApp>,
    rules: Vec<UiRuleConfig>,
    hotkey_warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UiStats {
    before_chars: usize,
    before_lines: usize,
    after_chars: usize,
    after_lines: usize,
    diff_added: usize,
    diff_removed: usize,
}

#[derive(Debug, Serialize)]
struct UiHotkeyApp {
    app: String,
    combo: String,
}

#[derive(Debug, Serialize)]
struct UiRuleConfig {
    id: String,
    name: String,
    description: String,
    pinned: bool,
}

#[derive(Debug, Serialize)]
struct UiHistoryItem {
    time: String,
    action: String,
    rule: String,
    snippet: String,
}

struct TrayHandle {
    _tray: TrayIcon,
    show_id: MenuId,
    quit_id: MenuId,
}

#[derive(Debug, Default)]
struct HotkeyRule {
    apps: Vec<String>,
    is_global: bool,
}

struct HotkeySpec {
    combo: String,
    app: Option<String>,
}

struct HotkeyEntry {
    hotkey: HotKey,
    combos: Vec<String>,
}

struct HistoryItem {
    time: DateTime<Utc>,
    action: String,
    rule: String,
    snippet: String,
}

#[derive(Debug)]
enum UserEvent {
    Ipc(IpcMessage),
    Menu(MenuEvent),
    Hotkey(u32),
}

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct Pasteflow {
    state: AppState,
    window: Option<Arc<Window>>,
    webview: Option<WebView>,
    tray: Option<TrayHandle>,
    proxy: EventLoopProxy<UserEvent>,
}

impl Pasteflow {
    fn new(proxy: EventLoopProxy<UserEvent>) -> AppResult<Self> {
        let cfg = config::load_or_init()?;
        let clipboard =
            Clipboard::new().map_err(|e| format!("Failed to access clipboard: {}", e))?;
        let hotkey_manager = GlobalHotKeyManager::new()?;

        let state = AppState {
            cfg,
            clipboard,
            suggestions: Vec::new(),
            selected_rule_id: None,
            panel: PanelState {
                input: String::new(),
                output: String::new(),
                diff: String::new(),
                error: None,
                active_app: None,
                content_types: Vec::new(),
                active_app_key: "global".to_string(),
                search_query: None,
            },
            config_text: None,
            config_error: None,
            config_draft_error: None,
            config_diff: None,
            hotkey_manager,
            registered_hotkeys: Vec::new(),
            hotkey_map: HashMap::new(),
            hotkey_warnings: Vec::new(),
            history: Vec::new(),
        };

        Ok(Self {
            state,
            window: None,
            webview: None,
            tray: None,
            proxy,
        })
    }
}

impl ApplicationHandler<UserEvent> for Pasteflow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Create window
        let window_attrs = Window::default_attributes()
            .with_title("Pasteflow")
            .with_visible(false)
            .with_inner_size(LogicalSize::new(900.0, 640.0));

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create window: {}", e);
                event_loop.exit();
                return;
            }
        };

        // Create webview as child to avoid winit contentView replacement panic
        // See: https://github.com/tauri-apps/wry/issues/1477
        let html = include_str!("../assets/panel.html");
        let proxy = self.proxy.clone();
        let webview = match WebViewBuilder::new()
            .with_html(html)
            .with_ipc_handler(move |req: Request<String>| {
                if let Ok(event) = serde_json::from_str::<IpcMessage>(req.body()) {
                    let _ = proxy.send_event(UserEvent::Ipc(event));
                }
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(900.0, 640.0)),
            })
            .build_as_child(&window)
        {
            Ok(wv) => wv,
            Err(e) => {
                eprintln!("Failed to create webview: {}", e);
                event_loop.exit();
                return;
            }
        };

        // Create tray
        let tray = match build_tray() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to create tray: {}", e);
                event_loop.exit();
                return;
            }
        };

        // Apply hotkeys
        let _ = apply_hotkeys(&mut self.state);

        // Start hotkey listener thread
        let hotkey_proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let rx = GlobalHotKeyEvent::receiver();
            while let Ok(event) = rx.recv() {
                let _ = hotkey_proxy.send_event(UserEvent::Hotkey(event.id()));
            }
        });

        // Start menu event listener thread
        let menu_proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let rx = MenuEvent::receiver();
            while let Ok(event) = rx.recv() {
                let _ = menu_proxy.send_event(UserEvent::Menu(event));
            }
        });

        self.window = Some(window);
        self.webview = Some(webview);
        self.tray = Some(tray);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(window) = &self.window {
                    window.set_visible(false);
                }
            }
            WindowEvent::Destroyed => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                // Resize webview to match window size
                if let Some(webview) = &self.webview {
                    let _ = webview.set_bounds(wry::Rect {
                        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(
                            0.0, 0.0,
                        )),
                        size: wry::dpi::Size::Physical(wry::dpi::PhysicalSize::new(
                            size.width,
                            size.height,
                        )),
                    });
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(window) = &self.window else { return };
        let Some(webview) = &self.webview else { return };
        let Some(tray) = &self.tray else { return };

        match event {
            UserEvent::Ipc(msg) => {
                handle_ipc(&mut self.state, msg, window, webview);
            }
            UserEvent::Menu(event) => {
                if event.id == tray.show_id {
                    open_panel(&mut self.state, window, webview);
                } else if event.id == tray.quit_id {
                    event_loop.exit();
                }
            }
            UserEvent::Hotkey(id) => {
                if should_handle_hotkey(&self.state, id) {
                    open_panel(&mut self.state, window, webview);
                }
            }
        }
    }
}

pub fn run() -> AppResult<()> {
    // On macOS, set activation policy to accessory (no dock icon)
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        use objc2_foundation::MainThreadMarker;

        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("Failed to create event loop: {}", e))?;

    let proxy = event_loop.create_proxy();
    let mut app = Pasteflow::new(proxy)?;

    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("Event loop error: {}", e))?;

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

    let icon = load_icon()?;
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

fn load_icon() -> Result<tray_icon::Icon, Box<dyn Error + Send + Sync>> {
    let bytes = include_bytes!("../assets/icon.png");
    let image =
        image::load_from_memory(bytes).map_err(|e| format!("Failed to decode icon: {}", e))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height)
        .map_err(|e| format!("Failed to create icon: {}", e).into())
}

fn open_panel(state: &mut AppState, window: &Window, webview: &WebView) {
    let text = match state.clipboard.get_text() {
        Ok(t) => t,
        Err(_) => {
            state.panel.error = Some("Failed to read clipboard".to_string());
            String::new()
        }
    };
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
    if let Some(selected) = &state.selected_rule_id
        && state.cfg.rules.iter().all(|rule| &rule.id != selected)
    {
        state.selected_rule_id = state
            .suggestions
            .first()
            .map(|suggestion| suggestion.rule.id.clone());
        update_ui_prefs(state, None, state.selected_rule_id.clone());
    }
    refresh_preview(state);

    if let Some(rule) = selected_rule(state)
        && rule.auto_accept
    {
        apply_paste(state);
        return;
    }

    send_state(state, webview);
    window.set_visible(true);
    window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
    window.focus_window();
}

fn handle_ipc(state: &mut AppState, msg: IpcMessage, window: &Window, webview: &WebView) {
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
                persist_config(state);
            }
            refresh_preview(state);
            send_state(state, webview);
        }
        IpcMessage::TogglePinned { id, value } => {
            if let Some(rule) = state.cfg.rules.iter_mut().find(|rule| rule.id == id) {
                rule.pinned = value;
                persist_config(state);
            }
            rebuild_suggestions(state);
            refresh_preview(state);
            send_state(state, webview);
        }
        IpcMessage::UpdateRuleDescription { id, value } => {
            if let Some(rule) = state.cfg.rules.iter_mut().find(|rule| rule.id == id) {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    rule.description = None;
                } else {
                    rule.description = Some(trimmed);
                }
                persist_config(state);
            }
            send_state(state, webview);
        }
        IpcMessage::UpdateHotkeyCombo { combo } => {
            let trimmed = combo.trim().to_string();
            if !trimmed.is_empty() {
                state.cfg.hotkey.combo = trimmed;
                persist_config(state);
                let _ = apply_hotkeys(state);
            }
            send_state(state, webview);
        }
        IpcMessage::UpdateHotkeyApp { app, combo } => {
            let app_trimmed = app.trim().to_string();
            let combo_trimmed = combo.trim().to_string();
            if !app_trimmed.is_empty() {
                if combo_trimmed.is_empty() {
                    state.cfg.hotkey.apps.remove(&app_trimmed);
                } else {
                    state.cfg.hotkey.apps.insert(app_trimmed, combo_trimmed);
                }
                persist_config(state);
                let _ = apply_hotkeys(state);
            }
            send_state(state, webview);
        }
        IpcMessage::RemoveHotkeyApp { app } => {
            state.cfg.hotkey.apps.remove(app.trim());
            persist_config(state);
            let _ = apply_hotkeys(state);
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
                    state.config_draft_error = None;
                    state.config_diff = None;
                }
                Err(err) => {
                    state.config_error = Some(err.to_string());
                }
            }
            send_state(state, webview);
        }
        IpcMessage::UpdateConfigDraft { raw } => {
            if let Some(saved) = state.config_text.as_deref() {
                if saved == raw {
                    state.config_diff = None;
                } else {
                    state.config_diff = Some(diff::unified_diff(saved, &raw));
                }
            } else {
                state.config_diff = None;
            }
            match config::parse_raw(&raw) {
                Ok(_) => {
                    state.config_draft_error = None;
                }
                Err(err) => {
                    state.config_draft_error = Some(err.to_string());
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
                        state.config_draft_error = None;
                        state.config_diff = None;
                        rebuild_suggestions(state);
                        refresh_preview(state);
                        let _ = apply_hotkeys(state);
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
    let result = if let Some(rule) = selected_rule(state) {
        apply_rule(rule, &input)
    } else {
        Ok(input.clone())
    };
    match result {
        Ok(output) => {
            state.panel.output = output.clone();
            state.panel.diff = diff::unified_diff(&input, &output);
            state.panel.error = None;
        }
        Err(err) => {
            state.panel.output = input.clone();
            state.panel.diff = diff::unified_diff(&input, &input);
            state.panel.error = Some(err);
        }
    }
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
    if let Some(id) = &state.selected_rule_id
        && state.cfg.rules.iter().all(|rule| &rule.id != id)
    {
        state.selected_rule_id = state
            .suggestions
            .first()
            .map(|suggestion| suggestion.rule.id.clone());
    }
}

fn apply_rule(rule: &Rule, input: &str) -> Result<String, String> {
    if let Some(kind) = rule.transform_kind() {
        match kind.apply(input) {
            Ok(out) => Ok(out),
            Err(err) => Err(format!("Transform error: {}", err)),
        }
    } else if rule.llm.is_some() {
        Err("LLM rule is configured but not enabled in this MVP.".to_string())
    } else {
        Ok(input.to_string())
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
    let ctx = current_match_context(state);
    let suggestions: Vec<UiRule> = state
        .suggestions
        .iter()
        .map(|suggestion| ui_rule_with_score(&suggestion.rule, suggestion.score))
        .collect();

    let all_rules: Vec<UiRule> = state
        .cfg
        .rules
        .iter()
        .map(|rule| {
            let score = rule_score(rule, &ctx);
            ui_rule_with_score(rule, score)
        })
        .collect();

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
        config: {
            let mut cfg = build_ui_config_state(&state.cfg);
            cfg.hotkey_warnings = state.hotkey_warnings.clone();
            cfg
        },
        config_text: state.config_text.clone(),
        config_error: state.config_error.clone(),
        config_draft_error: state.config_draft_error.clone(),
        config_diff: state.config_diff.clone(),
        history: state.history.iter().map(ui_history_item).collect(),
        stats: compute_stats(&state.panel),
        error: state.panel.error.clone(),
    };

    if let Ok(payload) = serde_json::to_string(&ui_state) {
        let script = format!("window.__SET_STATE__({});", payload);
        let _ = webview.evaluate_script(&script);
    }
}

fn build_ui_config_state(cfg: &config::Config) -> UiConfigState {
    let mut hotkey_apps: Vec<UiHotkeyApp> = cfg
        .hotkey
        .apps
        .iter()
        .map(|(app, combo)| UiHotkeyApp {
            app: app.clone(),
            combo: combo.clone(),
        })
        .collect();
    hotkey_apps.sort_by(|a, b| a.app.to_lowercase().cmp(&b.app.to_lowercase()));

    let rules = cfg
        .rules
        .iter()
        .map(|rule| UiRuleConfig {
            id: rule.id.clone(),
            name: rule.name.clone(),
            description: rule.description.clone().unwrap_or_default(),
            pinned: rule.pinned,
        })
        .collect();

    UiConfigState {
        hotkey_combo: cfg.hotkey.combo.clone(),
        hotkey_apps,
        rules,
        hotkey_warnings: Vec::new(),
    }
}

fn apply_copy(state: &mut AppState) {
    apply_copy_internal(state, "Copy");
}

fn apply_paste(state: &mut AppState) {
    apply_copy_internal(state, "Paste");
    // Simulate Cmd+V to paste - enigo 0.1 doesn't return errors
    // Small delays ensure key events are processed in order
    let mut enigo = Enigo::new();
    enigo.key_down(Key::Meta);
    std::thread::sleep(std::time::Duration::from_millis(10));
    enigo.key_click(Key::Layout('v'));
    std::thread::sleep(std::time::Duration::from_millis(10));
    enigo.key_up(Key::Meta);
}

fn apply_copy_internal(state: &mut AppState, action: &str) {
    if let Err(e) = state.clipboard.set_text(state.panel.output.clone()) {
        state.panel.error = Some(format!("Failed to copy: {}", e));
        return;
    }
    record_history(state, action);
}

fn record_history(state: &mut AppState, action: &str) {
    let rule_name = selected_rule(state)
        .map(|rule| rule.name.clone())
        .unwrap_or_else(|| "No rule".to_string());
    let snippet = snippet_text(&state.panel.output);
    state.history.insert(
        0,
        HistoryItem {
            time: Utc::now(),
            action: action.to_string(),
            rule: rule_name,
            snippet,
        },
    );
    if state.history.len() > 5 {
        state.history.truncate(5);
    }
}

fn snippet_text(text: &str) -> String {
    let mut cleaned = text.replace(['\n', '\r'], " ");
    while cleaned.contains("  ") {
        cleaned = cleaned.replace("  ", " ");
    }
    // Use char count to safely handle UTF-8 multi-byte characters
    if cleaned.chars().count() > 80 {
        let truncated: String = cleaned.chars().take(77).collect();
        format!("{}...", truncated)
    } else {
        cleaned
    }
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

fn ui_rule_with_score(rule: &Rule, score: i32) -> UiRule {
    UiRule {
        id: rule.id.clone(),
        name: rule.name.clone(),
        auto_accept: rule.auto_accept,
        uses_remote: rule.llm.is_some(),
        pinned: rule.pinned,
        score,
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
    if let Some(desc) = &rule.description
        && !desc.trim().is_empty()
    {
        base.push_str(" - ");
        base.push_str(desc.trim());
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

fn current_match_context(state: &AppState) -> MatchContext {
    MatchContext {
        text: state.panel.input.clone(),
        content_types: state.panel.content_types.clone(),
        active_app: state.panel.active_app.clone(),
    }
}

fn rule_score(rule: &Rule, ctx: &MatchContext) -> i32 {
    rule.matches(ctx).unwrap_or(0)
}

fn ui_history_item(item: &HistoryItem) -> UiHistoryItem {
    UiHistoryItem {
        time: item
            .time
            .with_timezone(&Local)
            .format("%H:%M:%S")
            .to_string(),
        action: item.action.clone(),
        rule: item.rule.clone(),
        snippet: item.snippet.clone(),
    }
}

fn compute_stats(panel: &PanelState) -> UiStats {
    let before_chars = panel.input.chars().count();
    let after_chars = panel.output.chars().count();
    let before_lines = if panel.input.is_empty() {
        0
    } else {
        panel.input.lines().count()
    };
    let after_lines = if panel.output.is_empty() {
        0
    } else {
        panel.output.lines().count()
    };
    let (diff_added, diff_removed) = diff_line_stats(&panel.diff);

    UiStats {
        before_chars,
        before_lines,
        after_chars,
        after_lines,
        diff_added,
        diff_removed,
    }
}

fn diff_line_stats(diff_text: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff_text.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
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
    persist_config(state);
}

fn persist_config(state: &mut AppState) {
    match config::save(&state.cfg) {
        Ok(_) => {
            state.config_error = None;
            state.config_text = config::load_raw().ok();
            state.config_diff = None;
            state.config_draft_error = None;
        }
        Err(err) => {
            state.config_error = Some(err.to_string());
        }
    }
}

fn active_app_name() -> Option<String> {
    match active_win_pos_rs::get_active_window() {
        Ok(window) => Some(window.app_name),
        Err(_) => None,
    }
}

fn should_handle_hotkey(state: &AppState, id: u32) -> bool {
    let Some(rule) = state.hotkey_map.get(&id) else {
        return true;
    };
    if rule.is_global || rule.apps.is_empty() {
        return true;
    }
    let active = active_app_name().unwrap_or_default().to_lowercase();
    if active.is_empty() {
        return false;
    }
    rule.apps
        .iter()
        .any(|app| active.contains(&app.to_lowercase()))
}

fn apply_hotkeys(state: &mut AppState) -> AppResult<()> {
    let specs = build_hotkey_specs(&state.cfg);
    let (entries, map, mut warnings) = build_hotkeys(specs);

    if entries.is_empty() {
        state.config_error = Some("No valid hotkeys registered.".to_string());
        return Err("no valid hotkeys registered".into());
    }

    if !state.registered_hotkeys.is_empty() {
        let _ = state
            .hotkey_manager
            .unregister_all(&state.registered_hotkeys);
    }

    let mut registered = Vec::new();
    for entry in entries {
        if state.hotkey_manager.register(entry.hotkey).is_ok() {
            registered.push(entry.hotkey);
        } else {
            warnings.push(format!(
                "Failed to register hotkey '{}'",
                entry.combos.join(", ")
            ));
        }
    }

    if registered.is_empty() {
        state.config_error = Some("Failed to register hotkeys.".to_string());
        return Err("failed to register hotkeys".into());
    }

    state.registered_hotkeys = registered;
    state.hotkey_map = map;
    state.hotkey_warnings = warnings;
    Ok(())
}

fn build_hotkey_specs(cfg: &config::Config) -> Vec<HotkeySpec> {
    let mut specs = Vec::new();
    specs.push(HotkeySpec {
        combo: cfg.hotkey.combo.clone(),
        app: None,
    });
    for (app, combo) in &cfg.hotkey.apps {
        specs.push(HotkeySpec {
            combo: combo.clone(),
            app: Some(app.clone()),
        });
    }
    specs
}

fn build_hotkeys(
    specs: Vec<HotkeySpec>,
) -> (Vec<HotkeyEntry>, HashMap<u32, HotkeyRule>, Vec<String>) {
    let mut hotkey_map: HashMap<u32, HotkeyRule> = HashMap::new();
    let mut hotkeys: HashMap<u32, HotkeyEntry> = HashMap::new();
    let mut warnings = Vec::new();

    for spec in specs {
        match parse_hotkey(&spec.combo) {
            Ok(hotkey) => {
                let entry = hotkey_map.entry(hotkey.id()).or_default();
                if let Some(app) = spec.app {
                    entry.apps.push(app);
                } else {
                    entry.is_global = true;
                }
                hotkeys
                    .entry(hotkey.id())
                    .and_modify(|existing| existing.combos.push(spec.combo.clone()))
                    .or_insert(HotkeyEntry {
                        hotkey,
                        combos: vec![spec.combo.clone()],
                    });
            }
            Err(err) => {
                warnings.push(format!("Hotkey '{}': {}", spec.combo, err));
            }
        }
    }

    for entry in hotkey_map.values_mut() {
        entry.apps.sort_by_key(|a| a.to_lowercase());
        entry.apps.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    }

    for (id, rule) in &hotkey_map {
        if rule.is_global && !rule.apps.is_empty() {
            if let Some(entry) = hotkeys.get(id) {
                warnings.push(format!(
                    "Hotkey '{}' is global and app-specific (apps: {}).",
                    entry.combos.join(", "),
                    rule.apps.join(", ")
                ));
            }
        } else if rule.apps.len() > 1
            && let Some(entry) = hotkeys.get(id)
        {
            warnings.push(format!(
                "Hotkey '{}' is shared by apps: {}.",
                entry.combos.join(", "),
                rule.apps.join(", ")
            ));
        }
    }

    (hotkeys.into_values().collect(), hotkey_map, warnings)
}

fn parse_hotkey(combo: &str) -> Result<HotKey, String> {
    let parts: Vec<&str> = combo
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("empty hotkey".to_string());
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
                code = code_from_key(key);
                if code.is_none() {
                    return Err(format!("unsupported key '{}'", key));
                }
            }
        }
    }

    let code = code.ok_or_else(|| "missing key".to_string())?;
    Ok(HotKey::new(Some(modifiers), code))
}

fn code_from_key(key: &str) -> Option<Code> {
    if key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(match ch.to_ascii_uppercase() {
                'A' => Code::KeyA,
                'B' => Code::KeyB,
                'C' => Code::KeyC,
                'D' => Code::KeyD,
                'E' => Code::KeyE,
                'F' => Code::KeyF,
                'G' => Code::KeyG,
                'H' => Code::KeyH,
                'I' => Code::KeyI,
                'J' => Code::KeyJ,
                'K' => Code::KeyK,
                'L' => Code::KeyL,
                'M' => Code::KeyM,
                'N' => Code::KeyN,
                'O' => Code::KeyO,
                'P' => Code::KeyP,
                'Q' => Code::KeyQ,
                'R' => Code::KeyR,
                'S' => Code::KeyS,
                'T' => Code::KeyT,
                'U' => Code::KeyU,
                'V' => Code::KeyV,
                'W' => Code::KeyW,
                'X' => Code::KeyX,
                'Y' => Code::KeyY,
                'Z' => Code::KeyZ,
                _ => return None,
            });
        }
        if ch.is_ascii_digit() {
            return Some(match ch {
                '0' => Code::Digit0,
                '1' => Code::Digit1,
                '2' => Code::Digit2,
                '3' => Code::Digit3,
                '4' => Code::Digit4,
                '5' => Code::Digit5,
                '6' => Code::Digit6,
                '7' => Code::Digit7,
                '8' => Code::Digit8,
                '9' => Code::Digit9,
                _ => return None,
            });
        }
    }

    match key {
        // Navigation & control
        "space" => Some(Code::Space),
        "tab" => Some(Code::Tab),
        "enter" | "return" => Some(Code::Enter),
        "esc" | "escape" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        "delete" => Some(Code::Delete),
        "up" | "arrowup" => Some(Code::ArrowUp),
        "down" | "arrowdown" => Some(Code::ArrowDown),
        "left" | "arrowleft" => Some(Code::ArrowLeft),
        "right" | "arrowright" => Some(Code::ArrowRight),
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" => Some(Code::PageUp),
        "pagedown" => Some(Code::PageDown),
        // Function keys
        "f1" => Some(Code::F1),
        "f2" => Some(Code::F2),
        "f3" => Some(Code::F3),
        "f4" => Some(Code::F4),
        "f5" => Some(Code::F5),
        "f6" => Some(Code::F6),
        "f7" => Some(Code::F7),
        "f8" => Some(Code::F8),
        "f9" => Some(Code::F9),
        "f10" => Some(Code::F10),
        "f11" => Some(Code::F11),
        "f12" => Some(Code::F12),
        // Symbols and punctuation
        "`" | "backquote" | "grave" => Some(Code::Backquote),
        "-" | "minus" => Some(Code::Minus),
        "=" | "equal" | "equals" => Some(Code::Equal),
        "[" | "bracketleft" => Some(Code::BracketLeft),
        "]" | "bracketright" => Some(Code::BracketRight),
        "\\" | "backslash" => Some(Code::Backslash),
        ";" | "semicolon" => Some(Code::Semicolon),
        "'" | "quote" => Some(Code::Quote),
        "," | "comma" => Some(Code::Comma),
        "." | "period" => Some(Code::Period),
        "/" | "slash" => Some(Code::Slash),
        _ => None,
    }
}
