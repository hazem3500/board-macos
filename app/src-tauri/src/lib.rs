//! Board for macOS. The main window is the same https://board.zem.cool/oz/ page (Cloudflare Access session in
//! WebKit's persistent store, no UI fork). Native parts: a menu-bar count with a dropdown you can answer from, Dock
//! badge, notifications with answer buttons and inline reply, ⌥⌘B Quick add panel, Work mode, board:// links,
//! Services, the app menu and auto-update. Reads and answers use the signed device routes (api.rs), paired from the
//! signed-in Board page; Quick add goes through the signed-in page itself, so it adds no new credential.
mod api;
mod demo;
pub mod logic;
#[cfg(target_os = "macos")]
mod mac;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::webview::PageLoadEvent;
use tauri::window::Color;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;

use crate::api::{Answer, Api, ApiError, Credential};
use crate::logic::{board_url, card_url, is_board, js_string, parse_link, parse_page_message, quick_add_body, short, Item, Link, Needs, PageMessage, Work};

const POLL: Duration = Duration::from_secs(30);
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 3600);
const MENU_ITEMS: usize = 8;
const QUICK_SHORTCUT: &str = "CmdOrCtrl+Alt+B";

static APP: OnceLock<AppHandle> = OnceLock::new();

fn user_agent() -> String {
    // Safari's own string, so sign-in pages treat the window like Safari, plus a token the Board page looks for.
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15 BoardMac/{}",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Default)]
struct State {
    needs: Option<Needs>,
    work: Option<Work>,
    offline: bool,
    /// Every card id seen this run (so a card is announced once), and the ones with a notification showing.
    ever: HashSet<String>,
    shown: HashSet<String>,
    seeded: bool,
    /// Cards by id, for answering from a notification after the list moved on.
    known: HashMap<String, Item>,
    /// What the tray menu lists, by index (menu ids carry the index, not the card id).
    menu: Vec<Item>,
    update: Option<String>,
    /// The page the main window last loaded, so page messages are only taken from the Board itself.
    page: Option<Url>,
    pair_prompted: bool,
    panel: Panel,
    zoom: f64,
}

#[derive(Clone, Default, Serialize)]
struct Panel {
    mode: String,
    prefill: String,
    card: Option<String>,
    title: String,
    text: String,
}

struct Shared {
    state: Mutex<State>,
    cred: Mutex<Option<Credential>>,
    api: Api,
    wake: tokio::sync::Notify,
    demo: bool,
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<i32>>>,
}

fn shared(app: &AppHandle) -> Arc<Shared> {
    app.state::<Arc<Shared>>().inner().clone()
}

fn paired(app: &AppHandle) -> bool {
    let s = shared(app);
    s.demo || s.cred.lock().unwrap().is_some()
}

// ---------- main window ----------

/// Glue for the Board page inside the app (only on board.zem.cool): links to other sites open in the default browser,
/// and the page's background colour reaches the native title bar so the two read as one surface.
const PAGE_SCRIPT: &str = r#"(() => {
  if (location.origin !== 'https://board.zem.cool') return;
  const send = path => location.assign('board-app://' + path);
  const outside = href => { try { const u = new URL(href, location.href); return /^(https?|mailto):$/.test(u.protocol) && u.origin !== location.origin ? u.href : null } catch { return null } };
  document.addEventListener('click', e => {
    const a = e.target instanceof Element ? e.target.closest('a[href]') : null;
    const u = a && outside(a.href);
    if (u) { e.preventDefault(); e.stopPropagation(); send('external?u=' + encodeURIComponent(u)) }
  }, true);
  const open = window.open;
  window.open = (url, ...rest) => { const u = outside(url); if (u) { send('external?u=' + encodeURIComponent(u)); return null } return open.call(window, url, ...rest) };
  let last = '';
  const tell = () => { const bg = getComputedStyle(document.body || document.documentElement).backgroundColor; if (bg && bg !== last) { last = bg; send('theme?bg=' + encodeURIComponent(bg)) } };
  addEventListener('DOMContentLoaded', () => {
    tell();
    new MutationObserver(() => requestAnimationFrame(tell)).observe(document.documentElement, { attributes: true });
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => requestAnimationFrame(tell));
  });
})();"#;

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn create_main(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    let handle = app.clone();
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(board_url("/oz/")))
        .title("Board")
        .inner_size(1200.0, 820.0)
        .min_inner_size(420.0, 520.0)
        .visible(visible)
        .user_agent(&user_agent())
        .initialization_script(PAGE_SCRIPT)
        .on_navigation(move |url| on_navigation(&handle, url))
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                page_loaded(window.app_handle(), payload.url().clone());
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Transparent).hidden_title(true);
    builder.build()
}

/// Every navigation in the main window. `board-app://` is the page talking to the app: handled here and cancelled, so
/// it never leaves the app. Web pages (the Board, Cloudflare Access and its sign-in pages) load in place; anything
/// else (mailto:, other apps' schemes) goes to macOS.
fn on_navigation(app: &AppHandle, url: &Url) -> bool {
    match url.scheme() {
        "board-app" => {
            let (app, url) = (app.clone(), url.clone());
            tauri::async_runtime::spawn(async move { page_message(&app, &url).await });
            false
        }
        "https" | "http" | "about" | "blob" | "data" | "tauri" => true,
        _ => {
            let _ = app.opener().open_url(url.as_str(), None::<&str>);
            false
        }
    }
}

fn page_loaded(app: &AppHandle, url: Url) {
    let s = shared(app);
    let on_board = is_board(&url) && url.path().starts_with("/oz");
    let ask_pair = {
        let mut state = s.state.lock().unwrap();
        state.page = Some(url.clone());
        let ask = on_board && !s.demo && s.cred.lock().unwrap().is_none() && !state.pair_prompted;
        if ask {
            state.pair_prompted = true;
        }
        ask
    };
    // Signed in but not connected yet: the Board page offers "Connect" (pwa.js), once per launch.
    if ask_pair && url.query_pairs().all(|(k, _)| k != "device") {
        start_pairing(app);
    }
}

fn start_pairing(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.navigate(board_url("/oz/?device=pair"));
        show_main(app);
    }
}

async fn page_message(app: &AppHandle, url: &Url) {
    let s = shared(app);
    let from_board = s.state.lock().unwrap().page.as_ref().is_some_and(is_board);
    if !from_board {
        return;
    }
    match parse_page_message(url) {
        PageMessage::Pair { code, client_id, client_secret } => {
            let name = computer_name();
            match s.api.pair(&code, &client_id, &client_secret, &name).await {
                Ok(cred) => {
                    *s.cred.lock().unwrap() = Some(cred);
                    s.state.lock().unwrap().seeded = false;
                    notify_permission(app);
                    note(app, "This Mac is connected", "Cards that need you now show in the menu bar and as notifications you can answer.");
                    s.wake.notify_one();
                }
                Err(e) => note(app, "Couldn't connect this Mac", &e.to_string()),
            }
        }
        PageMessage::External(target) => {
            let _ = app.opener().open_url(target.as_str(), None::<&str>);
        }
        PageMessage::Theme(r, g, b) => {
            if let Some(w) = main_window(app) {
                let _ = w.set_background_color(Some(Color(r, g, b, 255)));
            }
        }
        PageMessage::Done { request, status } => {
            if let Some(tx) = s.pending.lock().unwrap().remove(&request) {
                let _ = tx.send(status);
            }
        }
        PageMessage::Unknown => {}
    }
}

fn computer_name() -> String {
    std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn open_in_main(app: &AppHandle, url: Url) {
    if let Some(w) = main_window(app) {
        let _ = w.navigate(url);
    }
    show_main(app);
}

/// Runs a key the Board page already understands (n: new task, 1-4: views, /: search, ⌘K: commands).
fn page_key(app: &AppHandle, key: &str, meta: bool) {
    if let Some(w) = main_window(app) {
        let script = format!(
            "(() => {{ if (!{meta}) document.activeElement?.blur?.(); const t = document.activeElement || document.body; t.dispatchEvent(new KeyboardEvent('keydown', {{ key: {key}, metaKey: {meta}, bubbles: true, cancelable: true }})) }})()",
            key = js_string(key)
        );
        let _ = w.eval(script);
    }
    show_main(app);
}

// ---------- reading the Board ----------

async fn poll(app: AppHandle) {
    loop {
        refresh(&app).await;
        let s = shared(&app);
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            _ = s.wake.notified() => {}
        }
    }
}

async fn refresh(app: &AppHandle) {
    let s = shared(app);
    if s.demo {
        let (needs, work) = {
            let state = s.state.lock().unwrap();
            (state.needs.clone().unwrap_or_else(demo::needs), state.work.clone().unwrap_or_else(demo::work))
        };
        apply(app, needs, Some(work));
        return;
    }
    let cred = s.cred.lock().unwrap().clone();
    let Some(cred) = cred else {
        render(app);
        return;
    };
    match s.api.needs(&cred).await {
        Ok(needs) => {
            let work = s.api.work(&cred).await.ok();
            s.state.lock().unwrap().offline = false;
            apply(app, needs, work);
        }
        Err(ApiError::Revoked) => {
            api::forget();
            *s.cred.lock().unwrap() = None;
            let shown: Vec<String> = {
                let mut state = s.state.lock().unwrap();
                state.needs = None;
                state.work = None;
                state.pair_prompted = false;
                state.shown.drain().collect()
            };
            remove_notifications(app, shown);
            render(app);
        }
        Err(_) => {
            s.state.lock().unwrap().offline = true;
            render(app);
        }
    }
}

fn apply(app: &AppHandle, needs: Needs, work: Option<Work>) {
    let s = shared(app);
    let (fresh, gone) = {
        let mut state = s.state.lock().unwrap();
        for item in &needs.items {
            state.known.insert(item.id.clone(), item.clone());
        }
        let ids: HashSet<String> = needs.items.iter().map(|i| i.id.clone()).collect();
        let gone: Vec<String> = state.shown.iter().filter(|id| !ids.contains(*id)).cloned().collect();
        // The first read after launch or pairing only learns what's there (no burst of old cards), except in the demo.
        let fresh: Vec<Item> = if state.seeded || s.demo {
            needs.items.iter().filter(|i| !state.ever.contains(&i.id)).cloned().collect()
        } else {
            Vec::new()
        };
        state.seeded = true;
        state.ever.extend(ids.iter().cloned());
        for id in &gone {
            state.shown.remove(id);
        }
        state.shown.extend(fresh.iter().map(|i| i.id.clone()));
        state.needs = Some(needs.clone());
        if work.is_some() {
            state.work = work;
        }
        (fresh, gone)
    };
    #[cfg(target_os = "macos")]
    {
        let items = needs.items.clone();
        let _ = app.run_on_main_thread(move || {
            mac::set_categories(&items);
            for item in &fresh {
                mac::post(item);
            }
            mac::remove(&gone);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (fresh, gone);
    render(app);
}

fn remove_notifications(app: &AppHandle, ids: Vec<String>) {
    #[cfg(target_os = "macos")]
    let _ = app.run_on_main_thread(move || mac::remove(&ids));
    #[cfg(not(target_os = "macos"))]
    let _ = (app, ids);
}

fn note(app: &AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let (title, body) = (title.to_string(), body.to_string());
        let _ = app.run_on_main_thread(move || mac::note(&title, &body));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, title, body);
}

fn notify_permission(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.run_on_main_thread(mac::request_permission);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

// ---------- answering ----------

fn answer(app: &AppHandle, card: String, reply: Answer) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let s = shared(&app);
        let picked = match &reply {
            Answer::Option(o) => Some(o.clone()),
            Answer::Text(_) => None,
        };
        let result = if s.demo {
            Ok(())
        } else {
            let cred = s.cred.lock().unwrap().clone();
            match cred {
                Some(cred) => s.api.answer(&cred, &card, reply).await,
                None => Err(ApiError::NotPaired),
            }
        };
        match result {
            Ok(()) | Err(ApiError::Status(409, _)) => {
                {
                    let mut state = s.state.lock().unwrap();
                    if let Some(needs) = state.needs.as_mut() {
                        needs.items.retain(|i| i.id != card);
                        needs.count = needs.count.saturating_sub(1);
                    }
                    state.shown.remove(&card);
                }
                remove_notifications(&app, vec![card]);
                render(&app);
                if !s.demo {
                    s.wake.notify_one();
                }
            }
            Err(e) => {
                // Like Android: open the card with the option picked, one tap to send it from the Board page.
                note(&app, "Couldn't send your answer", &format!("{e}. The card is open in Board."));
                let app2 = app.clone();
                let url = card_url(&card, picked.as_deref());
                let _ = app.run_on_main_thread(move || open_in_main(&app2, url));
            }
        }
    });
}

/// Called by mac.rs from any thread.
#[cfg(target_os = "macos")]
pub(crate) fn on_notification(response: mac::Response) {
    let Some(app) = APP.get().cloned() else { return };
    let s = shared(&app);
    match response {
        mac::Response::Open(card) => {
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || open_in_main(&app2, card_url(&card, None)));
        }
        mac::Response::Option(card, index) => {
            let option = s.state.lock().unwrap().known.get(&card).and_then(|i| i.ordered_options().get(index).cloned());
            if let Some(option) = option {
                answer(&app, card, Answer::Option(option));
            }
        }
        mac::Response::Reply(card, text) => {
            let text = text.trim().to_string();
            if !text.is_empty() {
                answer(&app, card, Answer::Text(text));
            }
        }
    }
}

/// Services menu: "Add to Board" (selected text) or "Open in Board" (a selected link). Main thread.
#[cfg(target_os = "macos")]
pub(crate) fn on_service(open: bool, text: String) {
    let Some(app) = APP.get().cloned() else { return };
    if open {
        let link = text.split_whitespace().find(|w| w.starts_with("https://") || w.starts_with("board://")).unwrap_or("");
        handle_link(&app, link);
    } else {
        show_panel(&app, Panel { mode: "add".into(), prefill: short(&text, 500), ..Default::default() });
    }
}

// ---------- menu bar ----------

fn tray(app: &AppHandle) -> Option<TrayIcon> {
    app.tray_by_id("board")
}

fn render(app: &AppHandle) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(e) = render_now(&app2) {
            eprintln!("Board: menu update failed: {e}");
        }
    });
}

fn render_now(app: &AppHandle) -> tauri::Result<()> {
    let s = shared(app);
    let is_paired = paired(app);
    let (needs, work, offline, update) = {
        let state = s.state.lock().unwrap();
        (state.needs.clone(), state.work.clone(), state.offline, state.update.clone())
    };
    let count = needs.as_ref().map(|n| n.count).unwrap_or(0);
    let items: Vec<Item> = needs.as_ref().map(|n| n.items.iter().take(MENU_ITEMS).cloned().collect()).unwrap_or_default();

    let menu = Menu::new(app)?;
    let header = if !is_paired {
        "Connect this Mac to see what needs you".to_string()
    } else if needs.is_none() {
        "Reading the Board…".to_string()
    } else if count == 0 {
        "Nothing needs you".to_string()
    } else {
        format!("Waiting on you · {count}")
    };
    menu.append(&MenuItem::with_id(app, "header", header, false, None::<&str>)?)?;
    if offline && is_paired {
        menu.append(&MenuItem::with_id(app, "offline", "Offline: showing the last read", false, None::<&str>)?)?;
    }
    for (i, item) in items.iter().enumerate() {
        let sub = Submenu::with_id(app, format!("card:{i}"), short(&item.title, 56), true)?;
        let byline = item.byline();
        if !byline.is_empty() {
            sub.append(&MenuItem::with_id(app, format!("by:{i}"), short(&byline, 60), false, None::<&str>)?)?;
            sub.append(&PredefinedMenuItem::separator(app)?)?;
        }
        if item.is_choice() {
            for (j, option) in item.ordered_options().iter().enumerate() {
                let label = if Some(option) == item.recommend.as_ref() { format!("{option}  (recommended)") } else { option.clone() };
                sub.append(&MenuItem::with_id(app, format!("opt:{i}:{j}"), short(&label, 60), true, None::<&str>)?)?;
            }
            sub.append(&PredefinedMenuItem::separator(app)?)?;
        } else if item.is_text() {
            sub.append(&MenuItem::with_id(app, format!("reply:{i}"), "Reply…", true, None::<&str>)?)?;
            sub.append(&PredefinedMenuItem::separator(app)?)?;
        }
        sub.append(&MenuItem::with_id(app, format!("open:{i}"), "Open in Board", true, None::<&str>)?)?;
        menu.append(&sub)?;
    }
    if count > items.len() {
        menu.append(&MenuItem::with_id(app, "more", format!("{} more in Needs me…", count - items.len()), true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if let Some(work) = &work {
        menu.append(&CheckMenuItem::with_id(app, "work", "Work mode", true, work.on, None::<&str>)?)?;
        if let Some(label) = work.label.as_deref().filter(|l| !l.is_empty()) {
            menu.append(&MenuItem::with_id(app, "work-label", format!("   {label}"), false, None::<&str>)?)?;
        }
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, "quick", "Quick Add…", true, Some(QUICK_SHORTCUT))?)?;
    menu.append(&MenuItem::with_id(app, "show", "Open Board", true, None::<&str>)?)?;
    if !is_paired {
        menu.append(&MenuItem::with_id(app, "pair", "Connect This Mac…", true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    match &update {
        Some(v) => menu.append(&MenuItem::with_id(app, "restart", format!("Restart to Update to {v}"), true, None::<&str>)?)?,
        None => menu.append(&MenuItem::with_id(app, "check", "Check for Updates…", true, None::<&str>)?)?,
    }
    let login = app.autolaunch().is_enabled().unwrap_or(false);
    menu.append(&CheckMenuItem::with_id(app, "login", "Open at Login", true, login, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Board", true, Some("CmdOrCtrl+Q"))?)?;

    s.state.lock().unwrap().menu = items;
    if let Some(t) = tray(app) {
        t.set_menu(Some(menu))?;
        t.set_title(if is_paired && count > 0 { Some(count.to_string()) } else { None })?;
        t.set_tooltip(Some(if count > 0 { format!("Board: {count} waiting on you") } else { "Board".to_string() }))?;
    }
    if let Some(w) = main_window(app) {
        let _ = w.set_badge_count(if is_paired && count > 0 { Some(count as i64) } else { None });
    }
    Ok(())
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    let s = shared(app);
    let item_at = |i: &str| i.parse::<usize>().ok().and_then(|i| s.state.lock().unwrap().menu.get(i).cloned());
    let parts: Vec<&str> = id.split(':').collect();
    match parts.as_slice() {
        ["opt", i, j] => {
            if let (Some(item), Ok(j)) = (item_at(i), j.parse::<usize>()) {
                if let Some(option) = item.ordered_options().get(j).cloned() {
                    answer(app, item.id, Answer::Option(option));
                }
            }
        }
        ["reply", i] => {
            if let Some(item) = item_at(i) {
                show_panel(app, Panel { mode: "reply".into(), card: Some(item.id.clone()), title: item.title.clone(), text: item.text.clone().unwrap_or_default(), ..Default::default() });
            }
        }
        ["open", i] => {
            if let Some(item) = item_at(i) {
                open_in_main(app, card_url(&item.id, None));
            }
        }
        ["more"] | ["view", "needs"] => page_key(app, "1", false),
        ["view", "board"] => page_key(app, "2", false),
        ["view", "priorities"] => page_key(app, "3", false),
        ["view", "activity"] => page_key(app, "4", false),
        ["new-task"] => page_key(app, "n", false),
        ["find"] => page_key(app, "/", false),
        ["palette"] => page_key(app, "k", true),
        ["work"] => toggle_work(app),
        ["quick"] => show_panel(app, Panel { mode: "add".into(), ..Default::default() }),
        ["show"] => show_main(app),
        ["pair"] => {
            s.state.lock().unwrap().pair_prompted = true;
            start_pairing(app);
        }
        ["check"] => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { check_update(&app, true).await });
        }
        ["restart"] => app.restart(),
        ["login"] => {
            let auto = app.autolaunch();
            let _ = if auto.is_enabled().unwrap_or(false) { auto.disable() } else { auto.enable() };
            render(app);
        }
        ["quit"] => app.exit(0),
        ["reload"] => {
            if let Some(w) = main_window(app) {
                let _ = w.reload();
            }
        }
        ["zoom", how] => {
            let zoom = {
                let mut state = s.state.lock().unwrap();
                let z = if state.zoom == 0.0 { 1.0 } else { state.zoom };
                state.zoom = match *how {
                    "in" => (z + 0.1).min(2.0),
                    "out" => (z - 0.1).max(0.6),
                    _ => 1.0,
                };
                state.zoom
            };
            if let Some(w) = main_window(app) {
                let _ = w.set_zoom(zoom);
            }
        }
        ["help"] => {
            let _ = app.opener().open_url("https://github.com/hazem3500/board-macos#readme", None::<&str>);
        }
        _ => {}
    }
}

fn toggle_work(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let s = shared(&app);
        let on = !s.state.lock().unwrap().work.as_ref().map(|w| w.on).unwrap_or(false);
        let result = if s.demo {
            Ok(Work { on, manual: true, label: Some(if on { "On until Fri 0:00" } else { "Off until Sun 8:00" }.into()) })
        } else {
            let cred = s.cred.lock().unwrap().clone();
            match cred {
                Some(cred) => s.api.set_work(&cred, on).await,
                None => Err(ApiError::NotPaired),
            }
        };
        match result {
            Ok(work) => s.state.lock().unwrap().work = Some(work),
            Err(e) => note(&app, "Couldn't switch Work mode", &e.to_string()),
        }
        render(&app);
    });
}

// ---------- Quick add / reply panel ----------

fn panel_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window("quick") {
        return Ok(w);
    }
    let builder = WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("quick.html".into()))
        .title("Quick Add")
        .inner_size(620.0, 132.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .shadow(true)
        .visible(false)
        .center();
    #[cfg(target_os = "macos")]
    let builder = builder.effects(
        tauri::window::EffectsBuilder::new()
            .effect(tauri::window::Effect::Popover)
            .state(tauri::window::EffectState::Active)
            .radius(14.0)
            .build(),
    );
    builder.build()
}

fn show_panel(app: &AppHandle, panel: Panel) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let reply = panel.mode == "reply";
        shared(&app2).state.lock().unwrap().panel = panel;
        if let Ok(w) = panel_window(&app2) {
            let _ = w.set_size(tauri::LogicalSize::new(620.0, if reply { 300.0 } else { 132.0 }));
            let _ = w.center();
            let _ = w.show();
            let _ = w.set_focus();
            let _ = w.emit("panel", ());
            let _ = w.eval("window.boardPanel && window.boardPanel.refresh()");
        }
    });
}

fn toggle_panel(app: &AppHandle) {
    let visible = app.get_webview_window("quick").and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if visible {
        if let Some(w) = app.get_webview_window("quick") {
            let _ = w.hide();
        }
    } else {
        show_panel(app, Panel { mode: "add".into(), ..Default::default() });
    }
}

#[tauri::command]
fn panel_info(app: AppHandle) -> Panel {
    shared(&app).state.lock().unwrap().panel.clone()
}

#[tauri::command]
fn panel_hide(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
fn panel_open_board(app: AppHandle, window: WebviewWindow) {
    let _ = window.hide();
    show_main(&app);
}

#[tauri::command]
async fn panel_submit(app: AppHandle, text: String, note: String) -> Result<String, String> {
    let s = shared(&app);
    let panel = s.state.lock().unwrap().panel.clone();
    if panel.mode == "reply" {
        let card = panel.card.ok_or("No card")?;
        let text = text.trim().to_string();
        if text.is_empty() || text.chars().count() > 2000 {
            return Err("Write 1 to 2000 characters".into());
        }
        if text.contains("{{") {
            return Err("Answers can't contain “{{”".into());
        }
        if s.demo {
            return Ok("Sent".into());
        }
        let cred = s.cred.lock().unwrap().clone().ok_or("Connect this Mac first (menu bar → Connect This Mac…)")?;
        return match s.api.answer(&cred, &card, Answer::Text(text)).await {
            Ok(()) => {
                remove_notifications(&app, vec![card]);
                s.wake.notify_one();
                Ok("Sent".into())
            }
            Err(e) => Err(e.to_string()),
        };
    }
    quick_add(&app, &text, &note).await.map(|_| "Added to Inbox".to_string())
}

/// Adds a task through the signed-in Board page in the main window (its own Access session, the same request its Quick
/// add sends), so the app holds no owner credential. The page reports the status back through `board-app://done`.
async fn quick_add(app: &AppHandle, title: &str, note: &str) -> Result<(), String> {
    let s = shared(app);
    let body = quick_add_body(title, note, &format!("oz-ui-{}", uuid::Uuid::new_v4())).ok_or("Write a title (up to 500 characters)")?;
    if s.demo {
        return Ok(());
    }
    let on_board = s.state.lock().unwrap().page.as_ref().is_some_and(|u| is_board(u) && u.path().starts_with("/oz"));
    let Some(main) = main_window(app).filter(|_| on_board) else {
        show_main(app);
        return Err("Sign in to the Board first: it's open in the Board window.".into());
    };
    let request = uuid::Uuid::new_v4().simple().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    s.pending.lock().unwrap().insert(request.clone(), tx);
    let script = format!(
        "(async () => {{ let s = -1; try {{ const r = await fetch('/api/board/tasks', {{ method: 'POST', credentials: 'same-origin', redirect: 'manual', headers: {{ accept: 'application/json', 'content-type': 'application/json' }}, body: {body} }}); s = r.type === 'opaqueredirect' ? 0 : r.status }} catch {{}} location.assign('board-app://done?r={request}&s=' + s); if (s === 201 || s === 200) dispatchEvent(new Event('online')) }})()",
        body = js_string(&body.to_string())
    );
    main.eval(script).map_err(|e| e.to_string())?;
    let status = tokio::time::timeout(Duration::from_secs(15), rx).await;
    s.pending.lock().unwrap().remove(&request);
    match status {
        Ok(Ok(200 | 201)) => Ok(()),
        Ok(Ok(0 | 401 | 403)) => {
            show_main(app);
            Err("Your Board sign-in expired. Sign in again in the Board window.".into())
        }
        Ok(Ok(-1)) => Err("Can't reach the Board".into()),
        Ok(Ok(code)) => Err(format!("The Board answered {code}")),
        _ => Err("No answer from the Board".into()),
    }
}

// ---------- links, updates, app menu ----------

fn handle_link(app: &AppHandle, raw: &str) {
    match parse_link(raw) {
        Link::Show => show_main(app),
        Link::View(view) => match view.as_str() {
            "needs" => page_key(app, "1", false),
            "board" => page_key(app, "2", false),
            _ => open_in_main(app, board_url("/oz/#/projects")),
        },
        Link::Card(id) => open_in_main(app, card_url(&id, None)),
        Link::Add(text) => show_panel(app, Panel { mode: "add".into(), prefill: text, ..Default::default() }),
        Link::Open(url) => open_in_main(app, url),
        Link::Refused => {}
    }
}

async fn check_update(app: &AppHandle, manual: bool) {
    let Ok(updater) = app.updater() else { return };
    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            match update.download_and_install(|_, _| {}, || {}).await {
                Ok(()) => {
                    shared(app).state.lock().unwrap().update = Some(version.clone());
                    render(app);
                    note(app, &format!("Board {version} is ready"), "Restart Board from the menu bar to finish updating.");
                }
                Err(e) if manual => note(app, "Couldn't install the update", &e.to_string()),
                Err(_) => {}
            }
        }
        Ok(None) if manual => note(app, "Board is up to date", &format!("You have version {}.", env!("CARGO_PKG_VERSION"))),
        Err(e) if manual => note(app, "Couldn't check for updates", &e.to_string()),
        _ => {}
    }
}

fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let about = AboutMetadata {
        name: Some("Board".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        copyright: Some("oz-home".into()),
        website: Some("https://board.zem.cool/oz/".into()),
        ..Default::default()
    };
    let item = |id: &str, text: &str, accel: Option<&str>| MenuItem::with_id(app, id, text, true, accel);
    let board = Submenu::with_items(
        app,
        "Board",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Board"), Some(about))?,
            &item("check", "Check for Updates…", None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("Hide Board"))?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("Quit Board"))?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item("new-task", "New Task", Some("CmdOrCtrl+N"))?,
            &item("quick", "Quick Add…", Some(QUICK_SHORTCUT))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &item("find", "Find…", Some("CmdOrCtrl+F"))?,
            &item("palette", "Search or Jump To…", Some("CmdOrCtrl+K"))?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item("view:needs", "Needs Me", Some("CmdOrCtrl+1"))?,
            &item("view:board", "Board", Some("CmdOrCtrl+2"))?,
            &item("view:priorities", "Priorities", Some("CmdOrCtrl+3"))?,
            &item("view:activity", "Activity", Some("CmdOrCtrl+4"))?,
            &PredefinedMenuItem::separator(app)?,
            &item("reload", "Reload", Some("CmdOrCtrl+R"))?,
            &item("zoom:reset", "Actual Size", Some("CmdOrCtrl+0"))?,
            &item("zoom:in", "Zoom In", Some("CmdOrCtrl+Equal"))?,
            &item("zoom:out", "Zoom Out", Some("CmdOrCtrl+Minus"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::separator(app)?,
            &item("show", "Board", Some("CmdOrCtrl+Shift+0"))?,
        ],
    )?;
    let help = Submenu::with_items(app, "Help", true, &[&item("help", "Board Help", None)?])?;
    #[cfg(target_os = "macos")]
    {
        let _ = window.set_as_windows_menu_for_nsapp();
        let _ = help.set_as_help_menu_for_nsapp();
    }
    Menu::with_items(app, &[&board, &file, &edit, &view, &window, &help])
}

fn first_run(app: &AppHandle) -> bool {
    let Ok(dir) = app.path().app_config_dir() else { return false };
    let marker = dir.join("first-run-done");
    if marker.exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(marker, b"1").is_ok()
}

pub fn run() {
    let demo = std::env::var("BOARD_DEMO").as_deref() == Ok("1");
    let hidden = std::env::args().any(|a| a == "--hidden");
    let shared_state = Arc::new(Shared {
        state: Mutex::new(State { zoom: 1.0, ..Default::default() }),
        cred: Mutex::new(None),
        api: Api::new(&user_agent()),
        wake: tokio::sync::Notify::new(),
        demo,
        pending: Mutex::new(HashMap::new()),
    });

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["quick"])
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED
                        | tauri_plugin_window_state::StateFlags::FULLSCREEN,
                )
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(shared_state.clone())
        .invoke_handler(tauri::generate_handler![panel_info, panel_hide, panel_submit, panel_open_board])
        .on_menu_event(on_menu)
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                // Closing the window keeps Board in the menu bar; the Dock icon or ⌘⇧0 brings it back.
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(false) if window.label() == "quick" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            if !demo {
                *shared_state.cred.lock().unwrap() = api::load();
            }
            #[cfg(target_os = "macos")]
            {
                if let Some(mtm) = objc2::MainThreadMarker::new() {
                    mac::install(mtm);
                }
                if paired(&handle) {
                    mac::request_permission();
                }
            }
            app.set_menu(app_menu(&handle)?)?;
            TrayIconBuilder::with_id("board")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .show_menu_on_left_click(true)
                .menu(&Menu::new(app)?)
                .build(app)?;
            create_main(&handle, !hidden)?;
            panel_window(&handle)?;

            let shortcut_app = handle.clone();
            if let Err(e) = handle.global_shortcut().on_shortcut(QUICK_SHORTCUT, move |_app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    toggle_panel(&shortcut_app);
                }
            }) {
                eprintln!("Board: couldn't register {QUICK_SHORTCUT}: {e}");
            }

            let link_app = handle.clone();
            handle.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    handle_link(&link_app, url.as_str());
                }
            });
            if let Ok(Some(urls)) = handle.deep_link().get_current() {
                for url in urls {
                    handle_link(&handle, url.as_str());
                }
            }

            if first_run(&handle) && !demo {
                let _ = handle.autolaunch().enable();
            }

            render(&handle);
            tauri::async_runtime::spawn(poll(handle.clone()));
            if !demo {
                let update_app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                    loop {
                        check_update(&update_app, false).await;
                        tokio::time::sleep(UPDATE_EVERY).await;
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Board");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        let _ = (app, event);
    });
}
