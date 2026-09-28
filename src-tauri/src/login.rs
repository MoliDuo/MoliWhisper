//! Doubao login window and credential capture.
//!
//! The window is a plain WebView on www.doubao.com with no capability, so the
//! remote page cannot reach Tauri IPC. An injected script copies the two IDs
//! from localStorage into a marker cookie; a background task reads the cookie
//! store (httpOnly included) until a session shows up, checks it against the
//! ASR server, saves it and closes the window.

use std::time::Duration;

use moli_core::doubao::web::{
    ConnectOptions, Credentials, StoreCookie, Verdict, cookies_for_url, params, verify,
};
use tauri::webview::Cookie;
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::auth::Auth;
use crate::settings::Settings;
use crate::state_changed;

pub const LABEL: &str = "login";
const LOGIN_URL: &str = "https://www.doubao.com/chat/";
const PAGE_URL: &str = "https://www.doubao.com/";
const MARKER: &str = "__moliwhisper_ids";
const POLL: Duration = Duration::from_secs(1);
/// After the server rejects what the page has, give the user time to log in again.
const REJECTED_BACKOFF: Duration = Duration::from_secs(5);
/// Save anyway after this many checks that could not reach a verdict.
const MAX_INCONCLUSIVE: u32 = 3;

const SCRIPT: &str = r#"
(() => {
  if (location.hostname !== 'www.doubao.com') return;
  const pick = (key) => {
    try {
      const v = JSON.parse(localStorage.getItem(key) || 'null');
      return v && v.web_id != null ? String(v.web_id) : '';
    } catch { return ''; }
  };
  const ok = (s) => /^[A-Za-z0-9_-]{1,64}$/.test(s);
  const tick = () => {
    const d = pick('samantha_web_web_id');
    const w = pick('__tea_cache_tokens_497858');
    if (ok(d) && ok(w)) {
      document.cookie = `__MARKER__=${d}|${w}; path=/__moliwhisper; max-age=86400; secure; samesite=strict`;
    }
  };
  tick();
  setInterval(tick, 1000);
})();
"#;

/// Opens (or focuses) the login window. `fresh` wipes the WebView's data
/// first, for when the session it holds was rejected.
pub fn open<R: Runtime>(app: &AppHandle<R>, fresh: bool) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    let url = if fresh { "about:blank" } else { LOGIN_URL };
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url.parse().unwrap()))
        .title("登录豆包")
        .inner_size(1024.0, 760.0)
        .center()
        .focused(true)
        .initialization_script(SCRIPT.replace("__MARKER__", MARKER))
        .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            log::error!("could not open the login window: {e}");
            return;
        }
    };
    let _ = window.set_focus();
    log::info!("login window opened (fresh: {fresh})");

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if fresh {
            if let Err(e) = clear_browsing_data(&window).await {
                log::warn!("could not clear WebView data: {e}");
            }
            let _ = window.navigate(LOGIN_URL.parse().unwrap());
        }
        capture(app).await;
    });
}

async fn capture<R: Runtime>(app: AppHandle<R>) {
    let ws_url: Url = params::ENDPOINT.parse().unwrap();
    let page_url: Url = PAGE_URL.parse().unwrap();
    let mut inconclusive = 0;
    let mut last_state = None;

    loop {
        tokio::time::sleep(POLL).await;
        let Some(window) = app.get_webview_window(LABEL) else {
            log::info!("login window closed before a session was captured");
            return;
        };
        let cookies = match read_cookies(&window).await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("reading WebView cookies failed: {e}");
                continue;
            }
        };
        let Some(creds) = build_credentials(&cookies, &ws_url, &page_url) else {
            // Names only: enough to tell a missing marker from a missing session.
            let has = |name: &str| cookies.iter().any(|c| c.name() == name);
            let state = (has(MARKER), has("sessionid"), has("sid_guard"));
            if last_state != Some(state) {
                log::info!(
                    "waiting for the user to log in (marker: {}, sessionid: {}, sid_guard: {})",
                    state.0,
                    state.1,
                    state.2,
                );
                last_state = Some(state);
            }
            continue;
        };

        let overrides = app.state::<Settings>().get().asr.param_overrides;
        let verdict = verify(&ConnectOptions::new(&creds, &overrides)).await;
        match verdict {
            Verdict::Accepted => {}
            Verdict::Rejected => {
                log::warn!("the server rejected the session in the login window; waiting");
                tokio::time::sleep(REJECTED_BACKOFF).await;
                continue;
            }
            Verdict::Inconclusive(why) => {
                inconclusive += 1;
                if inconclusive < MAX_INCONCLUSIVE {
                    log::warn!("could not verify the session ({why}); retrying");
                    continue;
                }
                log::warn!("could not verify the session ({why}); saving it anyway");
            }
        }

        log::info!("captured {:?}", creds);
        let auth = app.state::<Auth>();
        if let Err(e) = auth.save(creds) {
            log::error!("could not save credentials: {e}");
            return;
        }
        state_changed(&app);
        let _ = window.destroy();
        return;
    }
}

/// Cookies from the whole store (httpOnly included). wry's `cookies_for_url`
/// on macOS only matches exact domains, so the matching is done here.
async fn read_cookies<R: Runtime>(
    window: &WebviewWindow<R>,
) -> Result<Vec<Cookie<'static>>, String> {
    // Blocks until the main thread answers; WebView2 deadlocks if this runs
    // on the main thread or in a synchronous handler.
    let window = window.clone();
    tauri::async_runtime::spawn_blocking(move || window.cookies())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

fn build_credentials(
    cookies: &[Cookie<'static>],
    ws_url: &Url,
    page_url: &Url,
) -> Option<Credentials> {
    let (device_id, web_id) = cookies
        .iter()
        .find(|c| c.name() == MARKER)
        .and_then(|c| c.value().split_once('|'))
        .map(|(d, w)| (d.to_string(), w.to_string()))?;
    let page = cookies_for_url(page_url, cookies.iter().map(as_store));
    let creds = Credentials {
        device_id,
        web_id,
        cookies: cookies_for_url(ws_url, cookies.iter().map(as_store)),
        language: page.get("i18next").cloned(),
        region: page.get("flow_user_country").cloned(),
    };
    creds.has_session().then_some(creds)
}

fn as_store<'a>(c: &'a Cookie<'static>) -> StoreCookie<'a> {
    StoreCookie {
        name: c.name(),
        value: c.value(),
        domain: c.domain().unwrap_or_default(),
        path: c.path().unwrap_or("/"),
        secure: c.secure().unwrap_or(false),
    }
}

async fn clear_browsing_data<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let window = window.clone();
    tauri::async_runtime::spawn_blocking(move || window.clear_all_browsing_data())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Forgets the credentials and the WebView's Doubao session.
pub async fn logout<R: Runtime>(app: AppHandle<R>) {
    if let Err(e) = app.state::<Auth>().logout() {
        log::error!("could not delete stored credentials: {e}");
    }
    // Clearing needs a webview; borrow the login window or make a hidden one.
    let window = match app.get_webview_window(LABEL) {
        Some(w) => Some(w),
        None => WebviewWindowBuilder::new(
            &app,
            LABEL,
            WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .visible(false)
        .build()
        .map_err(|e| log::error!("could not create a webview to clear data: {e}"))
        .ok(),
    };
    if let Some(window) = window {
        match clear_browsing_data(&window).await {
            Ok(()) => log::info!("logged out; WebView data cleared"),
            Err(e) => log::error!("could not clear WebView data: {e}"),
        }
        let _ = window.destroy();
    }
    state_changed(&app);
}
