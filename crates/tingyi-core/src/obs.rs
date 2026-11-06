//! OBS 瀏覽器來源用的本機網址:事件重播緩衝與路由判斷(HTTP 伺服器本體在 src-tauri)。

/// 本回合的管線事件(JSON 字串),讓中途才連上的 OBS 頁面先補齊畫面。
pub struct EventLog {
    cap: usize,
    events: std::collections::VecDeque<String>,
}

impl EventLog {
    pub fn new(cap: usize) -> Self {
        Self { cap, events: Default::default() }
    }

    pub fn push(&mut self, ev: String) {
        if self.events.len() >= self.cap {
            self.events.pop_front();
        }
        self.events.push_back(ev);
    }

    /// 新回合開始:清掉上一回合。
    pub fn reset(&mut self) {
        self.events.clear();
    }

    /// 上次 reset 之後的事件,舊到新;超過上限時丟最舊的。
    pub fn snapshot(&self) -> Vec<String> {
        self.events.iter().cloned().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/events`:SSE
    Events,
    /// `/history`、`/subtitle`:回 obs.html
    Page,
    /// 頁面用到的靜態檔(相對路徑,不含開頭 `/`)
    Asset(String),
    Forbidden,
    NotFound,
}

/// `target` 是 HTTP 請求行的路徑(可含 query);`host` 是 Host 標頭;`dev` 時頁面資源全部轉給 vite。
pub fn route(method: &str, target: &str, host: Option<&str>, dev: bool) -> Route {
    // 只收本機 Host:擋 DNS rebinding(外部網頁把自己的網域指到 127.0.0.1 來讀字幕)
    let name = host.map(|h| h.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(h, |(n, _)| n));
    if !matches!(name, Some("127.0.0.1" | "localhost")) {
        return Route::Forbidden;
    }
    if method != "GET" {
        return Route::NotFound;
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    match path {
        "/events" => return Route::Events,
        "/history" | "/subtitle" => return Route::Page,
        _ => {}
    }
    if path.contains("..") || path.contains('\\') {
        return Route::NotFound;
    }
    let rel = path.trim_start_matches('/');
    if dev {
        return if rel.is_empty() { Route::NotFound } else { Route::Asset(rel.to_string()) };
    }
    match rel.strip_prefix("assets/") {
        Some(name) if !name.is_empty() && !name.contains('/') => Route::Asset(rel.to_string()),
        _ => Route::NotFound,
    }
}
