use crate::config;
use crate::detect;
use crate::diff;
use crate::rules::{MatchContext, Rule, Suggestion};
use arboard::Clipboard;
use enigo::{Enigo, Key, KeyboardControllable};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use serde::{Deserialize, Serialize};
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
}

struct AppState {
    cfg: config::Config,
    clipboard: Clipboard,
    suggestions: Vec<Suggestion>,
    selected_rule_id: Option<String>,
    panel: PanelState,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcMessage {
    Paste,
    Copy,
    Cancel,
    SelectRule { id: String },
    ToggleAutoAccept { id: String, value: bool },
}

#[derive(Debug, Serialize)]
struct UiRule {
    id: String,
    name: String,
    auto_accept: bool,
    uses_remote: bool,
}

#[derive(Debug, Serialize)]
struct UiState {
    before: String,
    after: String,
    diff: String,
    rules: Vec<UiRule>,
    selected_rule_id: Option<String>,
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
        },
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
    let hotkey = parse_hotkey(&state.cfg.hotkey.combo).unwrap_or_else(default_hotkey);
    hotkey_manager.register(hotkey)?;

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
    let ctx = MatchContext {
        text: text.clone(),
        content_types,
        active_app,
    };
    state.suggestions = crate::rules::suggest_rules(&state.cfg.rules, &ctx, state.cfg.ui.suggestions);
    state.selected_rule_id = state
        .suggestions
        .first()
        .map(|suggestion| suggestion.rule.id.clone());
    state.panel.input = text;
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
    state
        .suggestions
        .iter()
        .find(|suggestion| suggestion.rule.id == id)
        .map(|suggestion| &suggestion.rule)
}

fn send_state(state: &AppState, webview: &WebView) {
    let ui_rules: Vec<UiRule> = state
        .suggestions
        .iter()
        .map(|suggestion| UiRule {
            id: suggestion.rule.id.clone(),
            name: suggestion.rule.name.clone(),
            auto_accept: suggestion.rule.auto_accept,
            uses_remote: suggestion.rule.llm.is_some(),
        })
        .collect();

    let ui_state = UiState {
        before: state.panel.input.clone(),
        after: state.panel.output.clone(),
        diff: state.panel.diff.clone(),
        rules: ui_rules,
        selected_rule_id: state.selected_rule_id.clone(),
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

fn active_app_name() -> Option<String> {
    match active_win_pos_rs::get_active_window() {
        Ok(window) => Some(window.app_name),
        Err(_) => None,
    }
}

fn default_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyV)
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
