//! Pure pieces of the Mac app, unit-tested on any platform: the device signature string, what changed between two
//! Needs-me reads, and which links and in-page messages the app accepts.
use std::collections::HashSet;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

/// The only Board the app talks to.
pub const ORIGIN: &str = "https://board.zem.cool";
pub const HOST: &str = "board.zem.cool";
/// The Cloudflare Access team host that serves the Board's sign-in (email code), and Cloudflare's challenge host its
/// page may embed. Nothing else loads in the main window.
pub const ACCESS_HOSTS: [&str; 2] = ["oz-cloud.cloudflareaccess.com", "challenges.cloudflare.com"];

/// What the main window does with a navigation (security review change 1).
#[derive(Debug, PartialEq)]
pub enum Nav {
    /// Load in place: the Board, its Access sign-in, about:/blob:/data:.
    Load,
    /// `board-app:`: the Board page talking to the app; handled and cancelled.
    Message,
    /// Any other web page: the default browser.
    Browser,
    /// mailto: only: handed to macOS.
    MacOs,
    /// Every other scheme (file:, smb:, other apps' schemes…): dropped.
    Drop,
}

pub fn navigation(url: &Url) -> Nav {
    match url.scheme() {
        "board-app" => Nav::Message,
        "about" | "blob" | "data" => Nav::Load,
        "https" if url.port().is_none() && (is_board(url) || url.host_str().is_some_and(|h| ACCESS_HOSTS.contains(&h))) => Nav::Load,
        "https" | "http" => Nav::Browser,
        "mailto" => Nav::MacOs,
        _ => Nav::Drop,
    }
}

/// One "Waiting on you" card as `/api/device/needs` and `/api/device/cards/:id` return it (devices.mjs `deviceItem`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub asker: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub recommend: Option<String>,
}

impl Item {
    /// Options with the recommended one first (the Board's own order otherwise).
    pub fn ordered_options(&self) -> Vec<String> {
        let mut options = self.options.clone().unwrap_or_default();
        if let Some(rec) = &self.recommend {
            if let Some(at) = options.iter().position(|o| o == rec) {
                let o = options.remove(at);
                options.insert(0, o);
            }
        }
        options
    }
    pub fn is_choice(&self) -> bool {
        self.kind == "choice" && self.options.as_ref().is_some_and(|o| o.len() >= 2)
    }
    pub fn is_text(&self) -> bool {
        self.kind == "text"
    }
    /// "Deploy-O-Bot · oz-home", for a notification's subtitle and the menu's detail line.
    pub fn byline(&self) -> String {
        [self.asker.as_deref(), self.project.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Needs {
    pub count: usize,
    pub items: Vec<Item>,
}

/// Work mode as `/api/device/work` returns it (work-mode.mjs `workMode`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Work {
    pub on: bool,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub label: Option<String>,
}

/// The string a device signs (devices.mjs `signedString`): method, path with query, time, nonce and the body's SHA-256.
pub fn signed_string(method: &str, path: &str, time: &str, nonce: &str, body: &[u8]) -> String {
    format!("{method}\n{path}\n{time}\n{nonce}\n{}", hex::encode(Sha256::digest(body)))
}

/// What to notify and what to take back after a read: cards not seen before, and notified cards no longer waiting.
pub fn diff(seen: &HashSet<String>, now: &[Item]) -> (Vec<Item>, Vec<String>) {
    let ids: HashSet<&str> = now.iter().map(|i| i.id.as_str()).collect();
    let fresh = now.iter().filter(|i| !seen.contains(&i.id)).cloned().collect();
    let mut gone: Vec<String> = seen.iter().filter(|id| !ids.contains(id.as_str())).cloned().collect();
    gone.sort();
    (fresh, gone)
}

/// A one-line menu title: whitespace folded, cut at `max` characters with an ellipsis.
pub fn short(text: &str, max: usize) -> String {
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= max {
        return folded;
    }
    let cut: String = folded.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Where a Board page lives, for the main window.
pub fn board_url(path_and_query: &str) -> Url {
    Url::parse(ORIGIN).unwrap().join(path_and_query).unwrap_or_else(|_| Url::parse(&format!("{ORIGIN}/oz/")).unwrap())
}

/// The Board page that opens one Needs-me card (the same link the Android app and web pushes use).
pub fn card_url(id: &str, pick: Option<&str>) -> Url {
    let mut url = board_url("/oz/");
    url.query_pairs_mut().append_pair("view", "needs").append_pair("card", id);
    if let Some(p) = pick {
        url.query_pairs_mut().append_pair("pick", p);
    }
    url
}

pub fn is_board(url: &Url) -> bool {
    url.scheme() == "https" && url.host_str() == Some(HOST) && url.port().is_none()
}

/// What a `board://` link (or a board.zem.cool link handed to the app) asks for.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// Bring the window forward.
    Show,
    /// One of the Board's views: needs, board or projects.
    View(String),
    Card(String),
    /// Quick add, with optional text to start from.
    Add(String),
    Open(Url),
    /// Refused: pairing only happens inside the signed-in Board page, never from a link another app can send.
    Refused,
}

const VIEWS: [&str; 3] = ["needs", "board", "projects"];

fn valid_card(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

/// board://needs, board://board, board://projects, board://card/<id>, board://add?text=…, board://open?url=<board link>,
/// or an https://board.zem.cool link itself.
pub fn parse_link(raw: &str) -> Link {
    let Ok(url) = Url::parse(raw) else { return Link::Show };
    if is_board(&url) {
        return Link::Open(url);
    }
    if url.scheme() != "board" {
        return Link::Refused;
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let query = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    match host.as_str() {
        "" => Link::Show,
        "pair" => Link::Refused,
        h if VIEWS.contains(&h) => Link::View(h.to_string()),
        "card" => {
            let id = url.path().trim_start_matches('/').to_string();
            let id = if id.is_empty() { query("id").unwrap_or_default() } else { id };
            if valid_card(&id) { Link::Card(id) } else { Link::Show }
        }
        "add" => Link::Add(short(&query("text").or_else(|| query("title")).unwrap_or_default(), 500)),
        "open" => match query("url").and_then(|u| Url::parse(&u).ok()) {
            Some(u) if is_board(&u) => Link::Open(u),
            _ => Link::Refused,
        },
        _ => Link::Show,
    }
}

/// A message the signed-in Board page sends the app by navigating to `board-app://…` (the navigation is caught and
/// cancelled inside the app's own web view, so nothing outside the app ever sees it).
#[derive(Debug, PartialEq)]
pub enum PageMessage {
    Pair { code: String, client_id: String, client_secret: String },
    External(Url),
    Theme(u8, u8, u8),
    Done { request: String, status: i32 },
    Unknown,
}

fn token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub fn parse_page_message(url: &Url) -> PageMessage {
    let query = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).unwrap_or_default();
    match url.host_str().unwrap_or("") {
        "pair" => {
            let (code, client_id, client_secret) = (query("code"), query("cid"), query("secret"));
            // The same shapes pwa-model.mjs pairIntentUrl accepts: a 256-bit base64url code and Access token parts.
            let code_ok = code.len() == 43 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if code_ok && token(&client_id, 1, 200) && token(&client_secret, 1, 200) {
                PageMessage::Pair { code, client_id, client_secret }
            } else {
                PageMessage::Unknown
            }
        }
        "external" => match Url::parse(&query("u")) {
            Ok(u) if ["http", "https", "mailto"].contains(&u.scheme()) => PageMessage::External(u),
            _ => PageMessage::Unknown,
        },
        "theme" => parse_rgb(&query("bg")).map(|(r, g, b)| PageMessage::Theme(r, g, b)).unwrap_or(PageMessage::Unknown),
        "done" => {
            let request = query("r");
            match query("s").parse::<i32>() {
                Ok(status) if token(&request, 8, 64) => PageMessage::Done { request, status },
                _ => PageMessage::Unknown,
            }
        }
        _ => PageMessage::Unknown,
    }
}

/// "rgb(9, 9, 11)" or "rgba(250, 250, 250, 1)" from getComputedStyle.
pub fn parse_rgb(css: &str) -> Option<(u8, u8, u8)> {
    let inner = css.trim().strip_prefix("rgba(").or_else(|| css.trim().strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts.len() == 4 && parts[3].parse::<f32>().ok()? < 0.5 {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
}

/// The text a JSON string literal needs inside an evaluated script.
pub fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// The body the Board's own Quick add sends (ui/model.mjs `quickAddBody`, no project or due date).
pub fn quick_add_body(title: &str, note: &str, id: &str) -> Option<serde_json::Value> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 500 || note.chars().count() > 4000 {
        return None;
    }
    let note = note.trim();
    let text = if note.is_empty() { title } else { format!("{title}\n\n{note}") };
    Some(serde_json::json!({ "channel": "personal/inbox", "text": text, "labels": { "status": "todo" }, "id": id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Item {
        Item { id: id.into(), title: id.into(), text: None, asker: None, project: None, kind: "task".into(), options: None, recommend: None }
    }

    #[test]
    fn signed_string_matches_the_server() {
        // devices.mjs: [method, path, time, nonce, sha256hex(body ?? '')].join('\n')
        let s = signed_string("GET", "/api/device/needs", "1700000000000", "abcdefghijklmnop", b"");
        assert_eq!(s, "GET\n/api/device/needs\n1700000000000\nabcdefghijklmnop\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn diff_finds_new_and_gone() {
        let seen: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let (fresh, gone) = diff(&seen, &[item("b"), item("c")]);
        assert_eq!(fresh.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["c"]);
        assert_eq!(gone, vec!["a".to_string()]);
    }

    #[test]
    fn recommended_option_comes_first() {
        let mut i = item("x");
        i.kind = "choice".into();
        i.options = Some(vec!["Later".into(), "Merge now".into(), "Close".into()]);
        i.recommend = Some("Merge now".into());
        assert_eq!(i.ordered_options(), vec!["Merge now", "Later", "Close"]);
        assert!(i.is_choice());
    }

    #[test]
    fn links() {
        assert_eq!(parse_link("board://needs"), Link::View("needs".into()));
        assert_eq!(parse_link("board://card/oz:task:12"), Link::Card("oz:task:12".into()));
        assert_eq!(parse_link("board://card/a%20b"), Link::Show);
        assert_eq!(parse_link("board://add?text=Buy%20milk"), Link::Add("Buy milk".into()));
        assert_eq!(parse_link("board://pair?code=x&cid=y&secret=z"), Link::Refused);
        assert_eq!(parse_link("board://open?url=https://evil.example/oz/"), Link::Refused);
        assert!(matches!(parse_link("board://open?url=https://board.zem.cool/oz/%23/board"), Link::Open(_)));
        assert!(matches!(parse_link("https://board.zem.cool/oz/?view=needs"), Link::Open(_)));
        assert_eq!(parse_link("https://board.zem.cool.evil.example/"), Link::Refused);
    }

    #[test]
    fn page_messages() {
        let code = "A".repeat(43);
        let ok = Url::parse(&format!("board-app://pair?code={code}&cid=abc.access&secret=s3cr3t-x")).unwrap();
        assert!(matches!(parse_page_message(&ok), PageMessage::Pair { .. }));
        let short_code = Url::parse("board-app://pair?code=abc&cid=a&secret=b").unwrap();
        assert_eq!(parse_page_message(&short_code), PageMessage::Unknown);
        let bad_secret = Url::parse(&format!("board-app://pair?code={code}&cid=a&secret=b%20c")).unwrap();
        assert_eq!(parse_page_message(&bad_secret), PageMessage::Unknown);
        let js = Url::parse("board-app://external?u=javascript:alert(1)").unwrap();
        assert_eq!(parse_page_message(&js), PageMessage::Unknown);
        let theme = Url::parse("board-app://theme?bg=rgb(9%2C%209%2C%2011)").unwrap();
        assert_eq!(parse_page_message(&theme), PageMessage::Theme(9, 9, 11));
        let done = Url::parse("board-app://done?r=abcdef0123&s=201").unwrap();
        assert_eq!(parse_page_message(&done), PageMessage::Done { request: "abcdef0123".into(), status: 201 });
    }

    #[test]
    fn quick_add_matches_the_web_form() {
        let body = quick_add_body("  Buy   milk ", "", "oz-ui-1").unwrap();
        assert_eq!(body["text"], "Buy milk");
        assert_eq!(body["channel"], "personal/inbox");
        assert_eq!(body["labels"]["status"], "todo");
        assert!(quick_add_body("   ", "", "x").is_none());
        assert_eq!(quick_add_body("T", " note ", "x").unwrap()["text"], "T\n\nnote");
    }

    #[test]
    fn main_window_navigation() {
        let nav = |u: &str| navigation(&Url::parse(u).unwrap());
        assert_eq!(nav("https://board.zem.cool/oz/"), Nav::Load);
        assert_eq!(nav("https://oz-cloud.cloudflareaccess.com/cdn-cgi/access/login/board.zem.cool"), Nav::Load);
        assert_eq!(nav("about:blank"), Nav::Load);
        assert_eq!(nav("board-app://theme?bg=x"), Nav::Message);
        assert_eq!(nav("file:///Applications/Calculator.app"), Nav::Drop);
        assert_eq!(nav("smb://x/y"), Nav::Drop);
        assert_eq!(nav("zoommtg://x"), Nav::Drop);
        assert_eq!(nav("x-apple.systempreferences:com.apple.preference.security"), Nav::Drop);
        assert_eq!(nav("tauri://localhost/"), Nav::Drop);
        assert_eq!(nav("https://evil.example/"), Nav::Browser);
        assert_eq!(nav("http://board.zem.cool/"), Nav::Browser);
        assert_eq!(nav("https://board.zem.cool.evil.example/"), Nav::Browser);
        assert_eq!(nav("https://board.zem.cool:8443/"), Nav::Browser);
        assert_eq!(nav("https://evil.cloudflareaccess.com/"), Nav::Browser);
        assert_eq!(nav("mailto:a@b.c"), Nav::MacOs);
    }

    #[test]
    fn short_titles() {
        assert_eq!(short("a  b\nc", 10), "a b c");
        assert_eq!(short("abcdefghij", 5), "abcd…");
    }
}
