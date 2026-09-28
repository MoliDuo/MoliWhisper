//! The recording overlay: created hidden at startup, shown at the top of the
//! screen under the cursor while a session runs, never focused.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use moli_core::session::{Outcome, Phase, Update};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::platform;

pub const LABEL: &str = "overlay";
const WIDTH: f64 = 440.0;
const HEIGHT: f64 = 64.0;
/// Gap below the menu bar, in points.
const TOP_GAP: f64 = 6.0;
const MESSAGE_FOR: Duration = Duration::from_millis(2200);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Msg<'a> {
    /// A new session: clear the previous text.
    Reset,
    /// About to be hidden: fade out and clear.
    Hide,
    State {
        state: &'static str,
    },
    Level {
        value: f32,
    },
    Text {
        text: &'a str,
    },
    Message {
        text: &'a str,
        error: bool,
    },
}

/// Bumped on every show, so a pending hide from an older session is skipped.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("MoliWhisper")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    let _ = window.set_ignore_cursor_events(true);
    platform::prepare_overlay(&window);
    Ok(())
}

pub fn update<R: Runtime>(app: &AppHandle<R>, update: &Update) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    match update {
        Update::Phase(Phase::Connecting) => {
            show(&window);
            emit(&window, Msg::Reset);
            emit(
                &window,
                Msg::State {
                    state: "connecting",
                },
            );
        }
        Update::Phase(Phase::Recording) => emit(&window, Msg::State { state: "recording" }),
        Update::Phase(Phase::Finalizing) => emit(
            &window,
            Msg::State {
                state: "finalizing",
            },
        ),
        Update::Phase(Phase::Delivering) => emit(
            &window,
            Msg::State {
                state: "delivering",
            },
        ),
        Update::Phase(Phase::Idle) => {}
        Update::Level(value) => emit(&window, Msg::Level { value: *value }),
        Update::Text(text) => emit(&window, Msg::Text { text }),
        Update::Outcome(outcome) => match message(outcome) {
            None => hide(&window),
            Some((text, error)) => {
                if !is_visible() {
                    show(&window);
                }
                emit(&window, Msg::Message { text, error });
                hide_later(window, MESSAGE_FOR);
            }
        },
    }
}

/// The transcript is being rewritten before it is pasted.
pub fn organizing<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        emit(
            &window,
            Msg::State {
                state: "organizing",
            },
        );
    }
}

fn message(outcome: &Outcome) -> Option<(&str, bool)> {
    Some(match outcome {
        Outcome::Done { partial: false } | Outcome::Cancelled => return None,
        Outcome::Done { partial: true } => ("连接中断，只识别了一部分", true),
        Outcome::Empty => ("没有听到内容", false),
        Outcome::NeedLogin => ("请先登录豆包", true),
        Outcome::SessionRejected => ("豆包登录已失效，请重新登录", true),
        Outcome::Network(_) => ("网络连接失败", true),
        Outcome::Microphone(_) => ("麦克风不可用", true),
        Outcome::DeliveryFailed(why) => (why.as_str(), true),
    })
}

static VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_visible() -> bool {
    VISIBLE.load(Ordering::Relaxed)
}

fn show<R: Runtime>(window: &WebviewWindow<R>) {
    GENERATION.fetch_add(1, Ordering::Relaxed);
    position(window);
    platform::show_overlay(window);
    VISIBLE.store(true, Ordering::Relaxed);
}

fn hide<R: Runtime>(window: &WebviewWindow<R>) {
    emit(window, Msg::Hide);
    platform::hide_overlay(window);
    VISIBLE.store(false, Ordering::Relaxed);
}

fn hide_later<R: Runtime>(window: WebviewWindow<R>, after: Duration) {
    let generation = GENERATION.load(Ordering::Relaxed);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(after).await;
        if GENERATION.load(Ordering::Relaxed) == generation {
            hide(&window);
        }
    });
}

/// Top centre of the work area of the monitor under the cursor.
fn position<R: Runtime>(window: &WebviewWindow<R>) {
    let monitor = window
        .cursor_position()
        .ok()
        .and_then(|p| window.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let _ = window.set_size(LogicalSize::new(WIDTH, HEIGHT));
    let width = (WIDTH * scale).round() as i32;
    let x = area.position.x + (area.size.width as i32 - width) / 2;
    let y = area.position.y + (TOP_GAP * scale).round() as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

fn emit<R: Runtime>(window: &WebviewWindow<R>, msg: Msg) {
    if let Err(e) = window.emit_to(LABEL, "overlay", msg) {
        log::debug!("overlay event dropped: {e}");
    }
}
