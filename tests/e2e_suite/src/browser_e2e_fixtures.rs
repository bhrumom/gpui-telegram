use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

// ============================================================================
// Shared Ephemeral Server Lifecycle & HTTP Helpers
// ============================================================================

struct MockServerHandle {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl MockServerHandle {
    async fn spawn(router: Router) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind ephemeral loopback mock server")?;
        let addr = listener.local_addr().context("read bound local_addr")?;
        let (tx, rx) = oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                let _ = rx.await;
            });
            let _ = server.await;
        });

        Ok(Self {
            addr,
            shutdown_tx: Some(tx),
            task: Some(task),
        })
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn url(&self, path: &str) -> String {
        let normalized = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("http://{}{}", self.addr, normalized)
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Build an HTTP client with ambient proxy disabled for deterministic `127.0.0.1` loopback calls.
pub fn no_proxy_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build no-proxy reqwest client")
}

/// Build a no-proxy HTTP client that does not follow redirects so `302` responses can be inspected.
pub fn no_redirect_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build no-redirect reqwest client")
}

/// Decode `application/x-www-form-urlencoded` payload into a `HashMap`.
pub fn parse_form_urlencoded(body: &[u8]) -> HashMap<String, String> {
    let raw = String::from_utf8_lossy(body);
    let mut out = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.insert(percent_decode_form(k), percent_decode_form(v));
    }
    out
}

fn percent_decode_form(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// Parse incoming Axum `Cookie` request headers into a key-value map.
pub fn parse_cookie_header(headers: &HeaderMap) -> HashMap<String, String> {
    let mut cookies = HashMap::new();
    for val in headers.get_all(COOKIE) {
        if let Ok(s) = val.to_str() {
            for part in s.split(';') {
                let trimmed = part.trim();
                if let Some((k, v)) = trimmed.split_once('=') {
                    let name = k.trim();
                    if !name.is_empty() {
                        cookies.insert(name.to_string(), v.trim().to_string());
                    }
                }
            }
        }
    }
    cookies
}

/// Parse `Set-Cookie` response headers from `reqwest` into a `name -> value` map.
pub fn parse_set_cookie_headers(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    let mut cookies = HashMap::new();
    for val in headers.get_all(reqwest::header::SET_COOKIE) {
        if let Ok(s) = val.to_str() {
            let first = s.split(';').next().unwrap_or("").trim();
            if let Some((k, v)) = first.split_once('=') {
                let name = k.trim();
                if !name.is_empty() {
                    cookies.insert(name.to_string(), v.trim().to_string());
                }
            }
        }
    }
    cookies
}

/// Escape HTML special characters for safe server-side rendering.
pub fn html_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn wants_json(headers: &HeaderMap, is_json_body: bool) -> bool {
    if is_json_body {
        return true;
    }
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.contains("application/json"))
}

fn is_json_content_type(headers: &HeaderMap, body: &[u8]) -> bool {
    if headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("application/json"))
    {
        return true;
    }
    let trimmed = body.iter().copied().find(|b| !b.is_ascii_whitespace());
    trimmed == Some(b'{')
}

// ============================================================================
// Mock X.com Social Media Server (`MockXServer`)
// ============================================================================

pub const DEFAULT_X_USERNAME: &str = "demo_operator";
pub const DEFAULT_X_PASSWORD: &str = "x-mock-pass-2026";
pub const DEFAULT_X_HANDLE: &str = "@demo_operator";
pub const DEFAULT_X_DISPLAY_NAME: &str = "Demo Operator";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockXSession {
    pub auth_token: String,
    pub ct0: String,
    pub user_id: String,
    pub username: String,
    pub handle: String,
    pub display_name: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockXTweet {
    pub id: String,
    pub author_handle: String,
    pub author_name: String,
    pub text: String,
    pub likes: u64,
    pub reposts: u64,
    #[serde(skip)]
    pub liked_by_tokens: HashSet<String>,
    #[serde(skip)]
    pub reposted_by_tokens: HashSet<String>,
    pub created_at_ms: i64,
}

pub struct MockXState {
    pub expected_username: String,
    pub expected_password: String,
    pub sessions: RwLock<HashMap<String, MockXSession>>,
    pub tweets: RwLock<Vec<MockXTweet>>,
    pub challenge_verified_tokens: RwLock<HashSet<String>>,
    next_tweet_id: AtomicU64,
}

impl MockXState {
    fn new(expected_username: String, expected_password: String) -> Self {
        let now = Utc::now().timestamp_millis();
        let initial_tweets = vec![
            MockXTweet {
                id: "1001".to_string(),
                author_handle: "@rust_bot".to_string(),
                author_name: "Rust Browser".to_string(),
                text: "Hello from local X mock".to_string(),
                likes: 0,
                reposts: 0,
                liked_by_tokens: HashSet::new(),
                reposted_by_tokens: HashSet::new(),
                created_at_ms: now - 20_000,
            },
            MockXTweet {
                id: "1002".to_string(),
                author_handle: "@automation_lab".to_string(),
                author_name: "Automation Lab".to_string(),
                text: "Testing browser automation workflows".to_string(),
                likes: 0,
                reposts: 0,
                liked_by_tokens: HashSet::new(),
                reposted_by_tokens: HashSet::new(),
                created_at_ms: now - 10_000,
            },
        ];

        Self {
            expected_username,
            expected_password,
            sessions: RwLock::new(HashMap::new()),
            tweets: RwLock::new(initial_tweets),
            challenge_verified_tokens: RwLock::new(HashSet::new()),
            next_tweet_id: AtomicU64::new(1003),
        }
    }
}

pub struct MockXServer {
    handle: MockServerHandle,
    state: Arc<MockXState>,
}

impl MockXServer {
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_credentials(DEFAULT_X_USERNAME, DEFAULT_X_PASSWORD).await
    }

    pub async fn start_with_credentials(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(MockXState::new(username.into(), password.into()));
        let router = Router::new()
            .route("/login", get(x_get_login).post(x_post_login))
            .route("/home", get(x_get_home))
            .route("/challenge", get(x_get_challenge).post(x_post_challenge))
            .route("/api/me", get(x_get_me))
            .route("/api/tweets", get(x_get_tweets).post(x_post_tweet))
            .route("/api/tweets/{id}/like", post(x_post_like_tweet))
            .route("/api/tweets/{id}/repost", post(x_post_repost_tweet))
            .with_state(Arc::clone(&state));

        let handle = MockServerHandle::spawn(router).await?;
        Ok(Self { handle, state })
    }

    pub fn addr(&self) -> SocketAddr {
        self.handle.addr()
    }

    pub fn port(&self) -> u16 {
        self.handle.addr().port()
    }

    pub fn base_url(&self) -> String {
        self.handle.base_url()
    }

    pub fn url(&self, path: &str) -> String {
        self.handle.url(path)
    }

    pub fn state(&self) -> &Arc<MockXState> {
        &self.state
    }

    /// Pre-seed an authenticated cookie session for cookie-reuse tests without calling `/login`.
    pub fn seed_session(
        &self,
        auth_token: impl Into<String>,
        ct0: impl Into<String>,
    ) -> MockXSession {
        let session = MockXSession {
            auth_token: auth_token.into(),
            ct0: ct0.into(),
            user_id: "u_1001".to_string(),
            username: self.state.expected_username.clone(),
            handle: DEFAULT_X_HANDLE.to_string(),
            display_name: DEFAULT_X_DISPLAY_NAME.to_string(),
            created_at_ms: Utc::now().timestamp_millis(),
        };
        self.state
            .sessions
            .write()
            .insert(session.auth_token.clone(), session.clone());
        session
    }

    pub fn tweet_count(&self) -> usize {
        self.state.tweets.read().len()
    }

    pub async fn shutdown(self) {
        self.handle.shutdown().await;
    }
}

fn authenticate_x(state: &MockXState, headers: &HeaderMap) -> Result<MockXSession, Response> {
    let cookies = parse_cookie_header(headers);
    let auth_token = cookies.get("auth_token").map(String::as_str).unwrap_or("");
    let ct0 = cookies.get("ct0").map(String::as_str).unwrap_or("");

    if auth_token.is_empty() || ct0.is_empty() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthenticated",
                "message": "Valid auth_token and ct0 cookies required"
            })),
        )
            .into_response());
    }

    let sessions = state.sessions.read();
    match sessions.get(auth_token) {
        Some(session) if session.ct0 == ct0 => Ok(session.clone()),
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthenticated",
                "message": "Invalid or expired auth_token/ct0 cookies"
            })),
        )
            .into_response()),
    }
}

async fn x_get_login() -> impl IntoResponse {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Sign in to X / Mock X</title>
</head>
<body>
  <main role="main">
    <h1>Sign in to X</h1>
    <div id="login-error" data-testid="login-error" role="alert" style="display:none;color:red;"></div>
    <form id="x-login-form" method="POST" action="/login" role="form" aria-label="Sign in to X">
      <label for="x-username">Phone, email, or username</label>
      <input id="x-username" name="username" type="text" data-testid="ocfEnterTextTextInput" aria-label="Phone, email, or username" role="textbox" autocomplete="username" />
      <label for="x-password">Password</label>
      <input id="x-password" name="password" type="password" data-testid="LoginForm_Password_Input" aria-label="Password" role="textbox" autocomplete="current-password" />
      <button id="x-login-btn" type="submit" data-testid="LoginForm_Login_Button" aria-label="Log in" role="button">Log in</button>
    </form>
  </main>
  <script>
    document.getElementById('x-login-form').addEventListener('submit', async function (ev) {
      ev.preventDefault();
      const username = document.getElementById('x-username').value;
      const password = document.getElementById('x-password').value;
      const resp = await fetch('/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username, password })
      });
      if (resp.ok) {
        window.location.href = '/home';
      } else {
        const err = await resp.json().catch(() => ({ message: 'Login failed' }));
        const box = document.getElementById('login-error');
        box.textContent = err.message || 'Login failed';
        box.style.display = 'block';
      }
    });
  </script>
</body>
</html>"#;
    Html(html)
}

#[derive(Deserialize)]
struct XLoginPayload {
    username: Option<String>,
    password: Option<String>,
}

async fn x_post_login(
    State(state): State<Arc<MockXState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (username, password) = if is_json_content_type(&headers, &body) {
        match serde_json::from_slice::<XLoginPayload>(&body) {
            Ok(payload) => (
                payload.username.unwrap_or_default(),
                payload.password.unwrap_or_default(),
            ),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "bad_request",
                        "message": "Invalid JSON login payload"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        let form = parse_form_urlencoded(&body);
        (
            form.get("username").cloned().unwrap_or_default(),
            form.get("password").cloned().unwrap_or_default(),
        )
    };

    if username.trim().is_empty() || password.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "bad_request",
                "message": "username and password are required"
            })),
        )
            .into_response();
    }

    if username != state.expected_username || password != state.expected_password {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "invalid_credentials",
                "message": "Invalid username or password"
            })),
        )
            .into_response();
    }

    let auth_token = format!("mock_x_auth_{}", Uuid::new_v4().simple());
    let ct0 = format!("mock_x_ct0_{}", Uuid::new_v4().simple());
    let session = MockXSession {
        auth_token: auth_token.clone(),
        ct0: ct0.clone(),
        user_id: "u_1001".to_string(),
        username: state.expected_username.clone(),
        handle: DEFAULT_X_HANDLE.to_string(),
        display_name: DEFAULT_X_DISPLAY_NAME.to_string(),
        created_at_ms: Utc::now().timestamp_millis(),
    };

    state
        .sessions
        .write()
        .insert(auth_token.clone(), session.clone());

    let mut resp = (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "user_id": session.user_id,
            "username": session.username,
            "handle": session.handle,
            "display_name": session.display_name,
            "auth_token": auth_token,
            "ct0": ct0,
        })),
    )
        .into_response();

    let cookie_auth = format!("{}= {}; Path=/; HttpOnly; SameSite=Lax", "auth_token", session.auth_token)
        .replace("= ", "=");
    let cookie_ct0 = format!("ct0={}; Path=/; SameSite=Lax", session.ct0);
    if let Ok(val) = HeaderValue::from_str(&cookie_auth) {
        resp.headers_mut().append(SET_COOKIE, val);
    }
    if let Ok(val) = HeaderValue::from_str(&cookie_ct0) {
        resp.headers_mut().append(SET_COOKIE, val);
    }
    resp
}

async fn x_get_home(State(state): State<Arc<MockXState>>, headers: HeaderMap) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(unauth_resp) => {
            if wants_json(&headers, false) {
                return unauth_resp;
            }
            return (StatusCode::FOUND, [(LOCATION, "/login")]).into_response();
        }
    };

    let tweets_html = {
        let tweets = state.tweets.read();
        let mut buf = String::new();
        for tweet in tweets.iter().rev() {
            let id = html_escape(&tweet.id);
            let author_name = html_escape(&tweet.author_name);
            let author_handle = html_escape(&tweet.author_handle);
            let text = html_escape(&tweet.text);
            buf.push_str(&format!(
                r#"<article data-testid="tweet" data-tweet-id="{id}" role="article" aria-label="Post by {author_handle}">
  <span data-testid="User-Name">{author_name} ({author_handle})</span>
  <div data-testid="tweetText">{text}</div>
  <button type="button" data-testid="like" data-tweet-id="{id}" aria-label="Like. {likes} Likes" role="button">Like ({likes})</button>
  <button type="button" data-testid="repost" data-tweet-id="{id}" aria-label="Repost. {reposts} Reposts" role="button">Repost ({reposts})</button>
</article>
"#,
                likes = tweet.likes,
                reposts = tweet.reposts,
            ));
        }
        buf
    };

    let handle_esc = html_escape(&session.handle);
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Home / X Mock</title>
</head>
<body>
  <header role="banner" aria-label="Primary">
    <span data-testid="user-handle">{handle_esc}</span>
    <a href="/challenge" data-testid="security-check-link" role="link">Security Check</a>
  </header>
  <main role="main">
    <section role="region" aria-label="Compose post">
      <textarea id="tweet-composer" data-testid="tweetTextarea_0" aria-label="Post text" role="textbox" placeholder="What is happening?!"></textarea>
      <button id="tweet-submit" data-testid="tweetButtonInline" aria-label="Post" role="button" type="button">Post</button>
    </section>
    <section id="timeline" data-testid="primaryColumn" role="region" aria-label="Timeline: Your Home Timeline">
      {tweets_html}
    </section>
  </main>
  <script>
    function attachTweetListeners(root) {{
      root.querySelectorAll('[data-testid="like"]').forEach(function (btn) {{
        btn.addEventListener('click', async function () {{
          const id = btn.getAttribute('data-tweet-id');
          const resp = await fetch('/api/tweets/' + encodeURIComponent(id) + '/like', {{ method: 'POST' }});
          if (resp.ok) {{
            const data = await resp.json();
            btn.textContent = 'Like (' + data.likes + ')';
            btn.setAttribute('aria-label', 'Like. ' + data.likes + ' Likes');
          }}
        }});
      }});
      root.querySelectorAll('[data-testid="repost"]').forEach(function (btn) {{
        btn.addEventListener('click', async function () {{
          const id = btn.getAttribute('data-tweet-id');
          const resp = await fetch('/api/tweets/' + encodeURIComponent(id) + '/repost', {{ method: 'POST' }});
          if (resp.ok) {{
            const data = await resp.json();
            btn.textContent = 'Repost (' + data.reposts + ')';
            btn.setAttribute('aria-label', 'Repost. ' + data.reposts + ' Reposts');
          }}
        }});
      }});
    }}
    attachTweetListeners(document);
    document.getElementById('tweet-submit').addEventListener('click', async function () {{
      const ta = document.getElementById('tweet-composer');
      const text = ta.value;
      const resp = await fetch('/api/tweets', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ text }})
      }});
      if (resp.ok) {{
        const data = await resp.json();
        const t = data.tweet;
        const article = document.createElement('article');
        article.setAttribute('data-testid', 'tweet');
        article.setAttribute('data-tweet-id', t.id);
        article.setAttribute('role', 'article');
        article.setAttribute('aria-label', 'Post by ' + t.author_handle);
        article.innerHTML =
          '<span data-testid="User-Name"></span>' +
          '<div data-testid="tweetText"></div>' +
          '<button type="button" data-testid="like" data-tweet-id="' + t.id + '" aria-label="Like. 0 Likes" role="button">Like (0)</button>' +
          '<button type="button" data-testid="repost" data-tweet-id="' + t.id + '" aria-label="Repost. 0 Reposts" role="button">Repost (0)</button>';
        article.querySelector('[data-testid="User-Name"]').textContent = t.author_name + ' (' + t.author_handle + ')';
        article.querySelector('[data-testid="tweetText"]').textContent = t.text;
        document.getElementById('timeline').prepend(article);
        attachTweetListeners(article);
        ta.value = '';
      }}
    }});
  </script>
</body>
</html>"#
    );

    Html(html).into_response()
}

async fn x_get_challenge(State(state): State<Arc<MockXState>>, headers: HeaderMap) -> Response {
    let cookies = parse_cookie_header(&headers);
    let verified = cookies
        .get("auth_token")
        .is_some_and(|tok| state.challenge_verified_tokens.read().contains(tok));
    let status_text = if verified {
        "Human verification complete"
    } else {
        "Verification required"
    };
    let checked_attr = if verified { "checked" } else { "" };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Security Checkpoint / X Mock</title>
</head>
<body>
  <main role="main">
    <h1>Verify you are human</h1>
    <div id="challenge-status" data-testid="challenge-status" role="status">{status_text}</div>
    <form id="challenge-form" action="/challenge" method="POST" role="form" aria-label="Human verification checkpoint">
      <label>
        <input id="human-checkbox" name="human_verified" type="checkbox" value="true" data-testid="human-captcha-checkbox" aria-label="Verify you are human" role="checkbox" {checked_attr} />
        I am human
      </label>
      <button type="submit" data-testid="challenge-continue-btn" aria-label="Continue" role="button">Continue</button>
    </form>
    <a href="/home" data-testid="back-to-home" role="link">Back to Home</a>
  </main>
  <script>
    document.getElementById('human-checkbox').addEventListener('change', async function (ev) {{
      if (ev.target.checked) {{
        const resp = await fetch('/challenge', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{ human_verified: true }})
        }});
        if (resp.ok) {{
          document.getElementById('challenge-status').textContent = 'Human verification complete';
        }}
      }}
    }});
  </script>
</body>
</html>"#
    );
    Html(html).into_response()
}

#[derive(Deserialize)]
struct ChallengePayload {
    human_verified: Option<bool>,
}

async fn x_post_challenge(
    State(state): State<Arc<MockXState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_json = is_json_content_type(&headers, &body);
    let verified = if is_json {
        serde_json::from_slice::<ChallengePayload>(&body)
            .ok()
            .and_then(|p| p.human_verified)
            .unwrap_or(false)
    } else {
        let form = parse_form_urlencoded(&body);
        matches!(
            form.get("human_verified").map(String::as_str),
            Some("true" | "1" | "on" | "yes")
        )
    };

    if !verified {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "verification_incomplete",
                "message": "Checkbox must be checked"
            })),
        )
            .into_response();
    }

    let cookies = parse_cookie_header(&headers);
    if let Some(tok) = cookies.get("auth_token") {
        state.challenge_verified_tokens.write().insert(tok.clone());
    }

    if wants_json(&headers, is_json) {
        (
            StatusCode::OK,
            Json(json!({ "ok": true, "verified": true })),
        )
            .into_response()
    } else {
        (StatusCode::FOUND, [(LOCATION, "/home")]).into_response()
    }
}

async fn x_get_me(State(state): State<Arc<MockXState>>, headers: HeaderMap) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    Json(json!({
        "user_id": session.user_id,
        "username": session.username,
        "handle": session.handle,
        "display_name": session.display_name,
    }))
    .into_response()
}

async fn x_get_tweets(State(state): State<Arc<MockXState>>, headers: HeaderMap) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let tweets = state.tweets.read();
    let list: Vec<serde_json::Value> = tweets
        .iter()
        .rev()
        .map(|t| {
            json!({
                "id": t.id,
                "author_handle": t.author_handle,
                "author_name": t.author_name,
                "text": t.text,
                "likes": t.likes,
                "reposts": t.reposts,
                "liked_by_me": t.liked_by_tokens.contains(&session.auth_token),
                "reposted_by_me": t.reposted_by_tokens.contains(&session.auth_token),
                "created_at_ms": t.created_at_ms,
            })
        })
        .collect();

    Json(json!({ "tweets": list })).into_response()
}

#[derive(Deserialize)]
struct CreateTweetPayload {
    text: Option<String>,
}

async fn x_post_tweet(
    State(state): State<Arc<MockXState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let payload: CreateTweetPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bad_request",
                    "message": "Invalid JSON body"
                })),
            )
                .into_response();
        }
    };

    let text = payload.text.unwrap_or_default();
    if text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "empty_tweet",
                "message": "Tweet text cannot be empty"
            })),
        )
            .into_response();
    }

    let id = state.next_tweet_id.fetch_add(1, Ordering::SeqCst).to_string();
    let tweet = MockXTweet {
        id,
        author_handle: session.handle,
        author_name: session.display_name,
        text: text.trim().to_string(),
        likes: 0,
        reposts: 0,
        liked_by_tokens: HashSet::new(),
        reposted_by_tokens: HashSet::new(),
        created_at_ms: Utc::now().timestamp_millis(),
    };

    state.tweets.write().push(tweet.clone());

    (
        StatusCode::CREATED,
        Json(json!({
            "ok": true,
            "tweet": {
                "id": tweet.id,
                "author_handle": tweet.author_handle,
                "author_name": tweet.author_name,
                "text": tweet.text,
                "likes": tweet.likes,
                "reposts": tweet.reposts,
                "created_at_ms": tweet.created_at_ms,
            }
        })),
    )
        .into_response()
}

async fn x_post_like_tweet(
    State(state): State<Arc<MockXState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let mut tweets = state.tweets.write();
    let Some(tweet) = tweets.iter_mut().find(|t| t.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "tweet_not_found",
                "message": format!("Tweet {id} not found")
            })),
        )
            .into_response();
    };

    let liked = if tweet.liked_by_tokens.insert(session.auth_token.clone()) {
        tweet.likes += 1;
        true
    } else {
        tweet.liked_by_tokens.remove(&session.auth_token);
        tweet.likes = tweet.likes.saturating_sub(1);
        false
    };

    Json(json!({
        "ok": true,
        "id": tweet.id,
        "likes": tweet.likes,
        "liked": liked,
    }))
    .into_response()
}

async fn x_post_repost_tweet(
    State(state): State<Arc<MockXState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session = match authenticate_x(&state, &headers) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let mut tweets = state.tweets.write();
    let Some(tweet) = tweets.iter_mut().find(|t| t.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "tweet_not_found",
                "message": format!("Tweet {id} not found")
            })),
        )
            .into_response();
    };

    let reposted = if tweet.reposted_by_tokens.insert(session.auth_token.clone()) {
        tweet.reposts += 1;
        true
    } else {
        tweet.reposted_by_tokens.remove(&session.auth_token);
        tweet.reposts = tweet.reposts.saturating_sub(1);
        false
    };

    Json(json!({
        "ok": true,
        "id": tweet.id,
        "reposts": tweet.reposts,
        "reposted": reposted,
    }))
    .into_response()
}

// ============================================================================
// Mock General Multi-Step Web Workflow Server (`MockWebAppServer`)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    pub title: String,
    pub category: String,
    pub price_cents: u64,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartItem {
    pub item_id: String,
    pub title: String,
    pub price_cents: u64,
    pub quantity: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRecord {
    pub confirmation_id: String,
    pub session_id: String,
    pub customer_name: String,
    pub customer_email: String,
    pub items: Vec<CartItem>,
    pub total_cents: u64,
}

pub struct MockWebAppState {
    pub catalog: Vec<CatalogItem>,
    pub carts: RwLock<HashMap<String, Vec<CartItem>>>,
    pub orders: RwLock<HashMap<String, OrderRecord>>,
}

impl MockWebAppState {
    fn new() -> Self {
        let catalog = vec![
            CatalogItem {
                id: "item-01".to_string(),
                title: "Rust Async Systems Handbook".to_string(),
                category: "books".to_string(),
                price_cents: 4500,
                description: "Comprehensive guide to Tokio, async traits, and zero-copy networking in Rust.".to_string(),
            },
            CatalogItem {
                id: "item-02".to_string(),
                title: "Browser Automation Engineering".to_string(),
                category: "books".to_string(),
                price_cents: 3900,
                description: "Deterministic DOM locators, CDP protocols, and resilient workflow distillation.".to_string(),
            },
            CatalogItem {
                id: "item-03".to_string(),
                title: "Rust Embedded WebView Patterns".to_string(),
                category: "books".to_string(),
                price_cents: 4200,
                description: "Building high-frame-rate desktop UIs with Rust and out-of-process WebViews.".to_string(),
            },
            CatalogItem {
                id: "item-04".to_string(),
                title: "Distributed SQLite Architecture".to_string(),
                category: "books".to_string(),
                price_cents: 3600,
                description: "WAL mode, crash recovery, and schema migrations for local-first desktop apps.".to_string(),
            },
            CatalogItem {
                id: "item-05".to_string(),
                title: "Mechanical Keycap Tester Kit".to_string(),
                category: "electronics".to_string(),
                price_cents: 2900,
                description: "Low-latency tactile switch array for Rust & Browser#1 automation QA benches.".to_string(),
            },
            CatalogItem {
                id: "item-06".to_string(),
                title: "USB-C Hardware Security Key".to_string(),
                category: "electronics".to_string(),
                price_cents: 5500,
                description: "FIDO2 hardware authenticator for local browser credential workflows.".to_string(),
            },
            CatalogItem {
                id: "item-07".to_string(),
                title: "4K Low-Latency Capture Dongle".to_string(),
                category: "electronics".to_string(),
                price_cents: 8900,
                description: "Hardware frame grabber for 120Hz UI compositing verification.".to_string(),
            },
            CatalogItem {
                id: "item-08".to_string(),
                title: "DOM Selector Inspector CLI".to_string(),
                category: "tools".to_string(),
                price_cents: 1900,
                description: "Command-line utility for Rust & Browser#1 ARIA roles and data-testid selectors.".to_string(),
            },
            CatalogItem {
                id: "item-09".to_string(),
                title: "Rust Flamegraph Profiler License".to_string(),
                category: "tools".to_string(),
                price_cents: 2500,
                description: "Interactive latency flamegraph tool for non-blocking Tokio pipelines.".to_string(),
            },
            CatalogItem {
                id: "item-10".to_string(),
                title: "Encrypted Cookie Vault Utility".to_string(),
                category: "tools".to_string(),
                price_cents: 2200,
                description: "Cross-profile session export and ChaCha20-Poly1305 backup helper.".to_string(),
            },
            CatalogItem {
                id: "item-11".to_string(),
                title: "WebKit Compositor Latency Benchmark".to_string(),
                category: "research".to_string(),
                price_cents: 1500,
                description: "Empirical dataset comparing WKWebView XPC and headless CDP throughput.".to_string(),
            },
            CatalogItem {
                id: "item-12".to_string(),
                title: "AI Agent Takeover UX Study".to_string(),
                category: "research".to_string(),
                price_cents: 1800,
                description: "Human-in-the-loop pause/resume patterns for browser automation agents.".to_string(),
            },
        ];

        Self {
            catalog,
            carts: RwLock::new(HashMap::new()),
            orders: RwLock::new(HashMap::new()),
        }
    }
}

pub struct MockWebAppServer {
    handle: MockServerHandle,
    state: Arc<MockWebAppState>,
}

impl MockWebAppServer {
    pub async fn start() -> anyhow::Result<Self> {
        let state = Arc::new(MockWebAppState::new());
        let router = Router::new()
            .route("/", get(webapp_get_index))
            .route("/search", get(webapp_get_search))
            .route("/item/{id}", get(webapp_get_item))
            .route("/cart/add", post(webapp_post_cart_add))
            .route("/checkout", get(webapp_get_checkout).post(webapp_post_checkout))
            .with_state(Arc::clone(&state));

        let handle = MockServerHandle::spawn(router).await?;
        Ok(Self { handle, state })
    }

    pub fn addr(&self) -> SocketAddr {
        self.handle.addr()
    }

    pub fn port(&self) -> u16 {
        self.handle.addr().port()
    }

    pub fn base_url(&self) -> String {
        self.handle.base_url()
    }

    pub fn url(&self, path: &str) -> String {
        self.handle.url(path)
    }

    pub fn state(&self) -> &Arc<MockWebAppState> {
        &self.state
    }

    pub fn order_count(&self) -> usize {
        self.state.orders.read().len()
    }

    pub async fn shutdown(self) {
        self.handle.shutdown().await;
    }
}

fn read_shop_sid(headers: &HeaderMap) -> Option<String> {
    let cookies = parse_cookie_header(headers);
    if let Some(sid) = cookies.get("shop_sid").filter(|s| !s.trim().is_empty()) {
        return Some(sid.clone());
    }
    headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn set_shop_sid_cookie(resp: &mut Response, sid: &str) {
    let header_str = format!("shop_sid={sid}; Path=/; SameSite=Lax");
    if let Ok(val) = HeaderValue::from_str(&header_str) {
        resp.headers_mut().append(SET_COOKIE, val);
    }
}

async fn webapp_get_index(headers: HeaderMap) -> Response {
    let existing_sid = read_shop_sid(&headers);
    let sid_to_set = if existing_sid.is_none() {
        Some(format!("sid_{}", Uuid::new_v4().simple()))
    } else {
        None
    };

    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Catalog Portal / Mock Web App</title>
</head>
<body>
  <header role="banner" aria-label="Storefront Header">
    <h1>Automation Catalog Portal</h1>
    <a href="/checkout" data-testid="go-to-checkout" role="link">Checkout</a>
  </header>
  <main role="main">
    <form action="/search" method="GET" role="search" aria-label="Catalog search">
      <label for="search-input">Search catalog</label>
      <input id="search-input" name="q" type="search" data-testid="search-input" aria-label="Search catalog" role="searchbox" placeholder="Search items..." />
      <label for="category-select">Category filter</label>
      <select id="category-select" name="category" data-testid="category-select" aria-label="Category filter" role="combobox">
        <option value="all">All Categories</option>
        <option value="books">Books</option>
        <option value="electronics">Electronics</option>
        <option value="tools">Tools</option>
        <option value="research">Research</option>
      </select>
      <input name="page_size" type="hidden" value="5" />
      <button type="submit" data-testid="search-submit" aria-label="Search" role="button">Search</button>
    </form>
  </main>
</body>
</html>"#;

    let mut resp = Html(html).into_response();
    if let Some(sid) = sid_to_set {
        set_shop_sid_cookie(&mut resp, &sid);
    }
    resp
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    category: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
    format: Option<String>,
}

async fn webapp_get_search(
    State(state): State<Arc<MockWebAppState>>,
    Query(params): Query<SearchParams>,
    headers: HeaderMap,
) -> Response {
    let q_raw = params.q.clone().unwrap_or_default();
    let q_lower = q_raw.trim().to_lowercase();
    let category_raw = params.category.clone().unwrap_or_else(|| "all".to_string());
    let category_lower = category_raw.trim().to_lowercase();
    let requested_page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(5).clamp(1, 50);

    let filtered: Vec<&CatalogItem> = state
        .catalog
        .iter()
        .filter(|item| {
            let cat_ok = category_lower.is_empty()
                || category_lower == "all"
                || item.category.to_lowercase() == category_lower;
            let q_ok = q_lower.is_empty()
                || item.title.to_lowercase().contains(&q_lower)
                || item.description.to_lowercase().contains(&q_lower);
            cat_ok && q_ok
        })
        .collect();

    let total_items = filtered.len();
    let total_pages = total_items.div_ceil(page_size).max(1);
    let page = requested_page.min(total_pages);
    let start = (page - 1).saturating_mul(page_size).min(total_items);
    let end = (start + page_size).min(total_items);
    let page_items: Vec<&CatalogItem> = filtered[start..end].to_vec();

    if wants_json(&headers, false) || params.format.as_deref() == Some("json") {
        return Json(json!({
            "items": page_items,
            "page": page,
            "page_size": page_size,
            "total_items": total_items,
            "total_pages": total_pages,
        }))
        .into_response();
    }

    let mut rows_html = String::new();
    for item in &page_items {
        let id = html_escape(&item.id);
        let title = html_escape(&item.title);
        let cat = html_escape(&item.category);
        let dollars = item.price_cents / 100;
        let cents = item.price_cents % 100;
        rows_html.push_str(&format!(
            r#"<tr data-testid="result-row" data-item-id="{id}" role="row">
  <td><a href="/item/{id}" data-testid="item-link-{id}" role="link">{title}</a></td>
  <td data-testid="item-category-{id}">{cat}</td>
  <td data-testid="item-price-{id}">${dollars}.{cents:02}</td>
</tr>
"#
        ));
    }

    let encoded_q = html_escape(&percent_encode_query(&q_raw));
    let encoded_category = html_escape(&percent_encode_query(&category_raw));

    let prev_link = if page > 1 {
        format!(
            r#"<a href="/search?q={}&amp;category={}&amp;page={}&amp;page_size={}" data-testid="prev-page" role="link" aria-label="Previous page">Previous</a>"#,
            encoded_q,
            encoded_category,
            page - 1,
            page_size
        )
    } else {
        String::new()
    };

    let next_link = if page < total_pages {
        format!(
            r#"<a href="/search?q={}&amp;category={}&amp;page={}&amp;page_size={}" data-testid="next-page" role="link" aria-label="Next page">Next</a>"#,
            encoded_q,
            encoded_category,
            page + 1,
            page_size
        )
    } else {
        String::new()
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Search Results / Mock Web App</title>
</head>
<body>
  <main role="main">
    <h1>Search Results</h1>
    <span data-testid="total-results">{total_items} results</span>
    <table data-testid="results-table" role="table" aria-label="Search results">
      <thead>
        <tr role="row"><th>Item</th><th>Category</th><th>Price</th></tr>
      </thead>
      <tbody>
        {rows_html}
      </tbody>
    </table>
    <nav data-testid="pagination" role="navigation" aria-label="Pagination">
      {prev_link}
      <span data-testid="page-indicator">Page {page} of {total_pages}</span>
      {next_link}
    </nav>
  </main>
</body>
</html>"#
    );

    Html(html).into_response()
}

async fn webapp_get_item(
    State(state): State<Arc<MockWebAppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(item) = state.catalog.iter().find(|it| it.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Html(r#"<!DOCTYPE html><html><head><title>Not Found</title></head><body><h1 data-testid="not-found">Item not found</h1></body></html>"#),
        )
            .into_response();
    };

    let item_id = html_escape(&item.id);
    let title = html_escape(&item.title);
    let category = html_escape(&item.category);
    let description = html_escape(&item.description);
    let dollars = item.price_cents / 100;
    let cents = item.price_cents % 100;

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>{title} / Mock Web App</title>
</head>
<body>
  <main role="main">
    <h1 data-testid="item-title">{title}</h1>
    <span data-testid="item-category">{category}</span>
    <span data-testid="item-price">${dollars}.{cents:02}</span>
    <p data-testid="item-description">{description}</p>
    <form action="/cart/add" method="POST" role="form" aria-label="Add item to cart">
      <input type="hidden" name="item_id" value="{item_id}" />
      <label for="qty-input">Quantity</label>
      <input id="qty-input" name="quantity" type="number" value="1" min="1" data-testid="qty-input" aria-label="Quantity" role="spinbutton" />
      <button type="submit" data-testid="add-to-cart-btn" aria-label="Add to cart" role="button">Add to cart</button>
    </form>
    <a href="/checkout" data-testid="go-to-checkout" role="link">Proceed to checkout</a>
  </main>
</body>
</html>"#
    );

    Html(html).into_response()
}

#[derive(Deserialize)]
struct CartAddJson {
    item_id: Option<String>,
    quantity: Option<u32>,
}

async fn webapp_post_cart_add(
    State(state): State<Arc<MockWebAppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_json = is_json_content_type(&headers, &body);
    let (item_id, quantity): (String, u32) = if is_json {
        let parsed: CartAddJson = match serde_json::from_slice(&body) {
            Ok(p) => p,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "bad_request", "message": "Invalid JSON" })),
                )
                    .into_response();
            }
        };
        let qty = parsed.quantity.unwrap_or(1);
        (parsed.item_id.unwrap_or_default(), qty)
    } else {
        let form = parse_form_urlencoded(&body);
        let id = form.get("item_id").cloned().unwrap_or_default();
        let qty = match form.get("quantity") {
            None => 1,
            Some(s) => match s.trim().parse::<u32>() {
                Ok(n) => n,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "invalid_quantity", "message": "Quantity must be a positive integer" })),
                    )
                        .into_response();
                }
            },
        };
        (id, qty)
    };

    if item_id.trim().is_empty() || quantity == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "invalid_cart_item",
                "message": "Non-empty item_id and quantity >= 1 required"
            })),
        )
            .into_response();
    }

    let Some(catalog_item) = state.catalog.iter().find(|it| it.id == item_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": "item_not_found",
                "message": format!("Catalog item {item_id} not found")
            })),
        )
            .into_response();
    };

    let sid = read_shop_sid(&headers).unwrap_or_else(|| format!("sid_{}", Uuid::new_v4().simple()));

    let cart_snapshot = {
        let mut carts = state.carts.write();
        let cart = carts.entry(sid.clone()).or_default();
        if let Some(existing) = cart.iter_mut().find(|c| c.item_id == catalog_item.id) {
            existing.quantity = existing.quantity.saturating_add(quantity);
        } else {
            cart.push(CartItem {
                item_id: catalog_item.id.clone(),
                title: catalog_item.title.clone(),
                price_cents: catalog_item.price_cents,
                quantity,
            });
        }
        cart.clone()
    };

    let total_qty: u32 = cart_snapshot.iter().map(|c| c.quantity).sum();

    let mut resp = if wants_json(&headers, is_json) {
        (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "session_id": sid,
                "cart_count": total_qty,
                "items": cart_snapshot,
            })),
        )
            .into_response()
    } else {
        let title_esc = html_escape(&catalog_item.title);
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8" /><title>Cart Updated</title></head>
<body>
  <div data-testid="cart-status" role="status">Added {title_esc}. Cart items: {total_qty}</div>
  <a href="/checkout" data-testid="go-to-checkout" role="link">Proceed to checkout</a>
</body>
</html>"#
        );
        (StatusCode::OK, Html(html)).into_response()
    };

    set_shop_sid_cookie(&mut resp, &sid);
    resp
}

async fn webapp_get_checkout(
    State(state): State<Arc<MockWebAppState>>,
    headers: HeaderMap,
) -> Response {
    let items = read_shop_sid(&headers)
        .and_then(|sid| state.carts.read().get(&sid).cloned())
        .unwrap_or_default();
    let total_qty: u32 = items.iter().map(|i| i.quantity).sum();
    let total_cents: u64 = items
        .iter()
        .map(|i| i.price_cents.saturating_mul(u64::from(i.quantity)))
        .sum();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Checkout / Mock Web App</title>
</head>
<body>
  <main role="main">
    <h1>Checkout</h1>
    <section data-testid="cart-summary" role="region" aria-label="Cart summary">
      <span data-testid="cart-item-count">{total_qty}</span>
      <span data-testid="cart-total-cents">{total_cents}</span>
    </section>
    <form id="checkout-form" action="/checkout" method="POST" role="form" aria-label="Checkout form" novalidate>
      <label for="checkout-name">Full name</label>
      <input id="checkout-name" name="name" type="text" data-testid="checkout-name" aria-label="Full name" role="textbox" />
      <label for="checkout-email">Email address</label>
      <input id="checkout-email" name="email" type="email" data-testid="checkout-email" aria-label="Email address" role="textbox" />
      <button type="submit" data-testid="checkout-submit" aria-label="Submit order" role="button">Submit order</button>
    </form>
  </main>
</body>
</html>"#
    );

    Html(html).into_response()
}

#[derive(Deserialize)]
struct CheckoutJson {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ValidationErrorItem {
    field: &'static str,
    message: &'static str,
}

fn is_valid_email(email: &str) -> bool {
    let trimmed = email.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = trimmed.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return false;
    }
    let mut segments = domain.split('.');
    let mut count = 0;
    for seg in &mut segments {
        if seg.is_empty() {
            return false;
        }
        count += 1;
    }
    count >= 2
}

async fn webapp_post_checkout(
    State(state): State<Arc<MockWebAppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_json = is_json_content_type(&headers, &body);
    let (name, email) = if is_json {
        match serde_json::from_slice::<CheckoutJson>(&body) {
            Ok(p) => (p.name.unwrap_or_default(), p.email.unwrap_or_default()),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "bad_request", "message": "Invalid JSON body" })),
                )
                    .into_response();
            }
        }
    } else {
        let form = parse_form_urlencoded(&body);
        (
            form.get("name").cloned().unwrap_or_default(),
            form.get("email").cloned().unwrap_or_default(),
        )
    };

    let sid_opt = read_shop_sid(&headers);
    let cart_items = sid_opt
        .as_ref()
        .and_then(|sid| state.carts.read().get(sid).cloned())
        .unwrap_or_default();

    let mut errors: Vec<ValidationErrorItem> = Vec::new();
    if cart_items.is_empty() {
        errors.push(ValidationErrorItem {
            field: "cart",
            message: "Cart cannot be empty",
        });
    }
    if name.trim().is_empty() {
        errors.push(ValidationErrorItem {
            field: "name",
            message: "Customer name is required",
        });
    }
    if !is_valid_email(&email) {
        errors.push(ValidationErrorItem {
            field: "email",
            message: "Valid email address is required",
        });
    }

    if !errors.is_empty() {
        if wants_json(&headers, is_json) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "ok": false,
                    "errors": errors,
                })),
            )
                .into_response();
        }
        let mut list_html = String::new();
        for err in &errors {
            list_html.push_str(&format!(
                r#"<li data-field="{}">{}</li>"#,
                html_escape(err.field),
                html_escape(err.message)
            ));
        }
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Html(format!(
                r#"<!DOCTYPE html><html><head><title>Checkout Validation Error</title></head><body><ul data-testid="validation-errors" role="alert">{list_html}</ul></body></html>"#
            )),
        )
            .into_response();
    }

    let sid = sid_opt.expect("cart non-empty implies session id present");
    let drained_items = state
        .carts
        .write()
        .remove(&sid)
        .unwrap_or(cart_items);
    let total_cents: u64 = drained_items
        .iter()
        .map(|i| i.price_cents.saturating_mul(u64::from(i.quantity)))
        .sum();
    let item_count: u32 = drained_items.iter().map(|i| i.quantity).sum();
    let confirmation_id = format!(
        "ORD-{}",
        &Uuid::new_v4().simple().to_string()[..8].to_uppercase()
    );

    let record = OrderRecord {
        confirmation_id: confirmation_id.clone(),
        session_id: sid,
        customer_name: name.trim().to_string(),
        customer_email: email.trim().to_string(),
        items: drained_items,
        total_cents,
    };
    state
        .orders
        .write()
        .insert(confirmation_id.clone(), record);

    if wants_json(&headers, is_json) {
        (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "confirmation_id": confirmation_id,
                "total_cents": total_cents,
                "item_count": item_count,
            })),
        )
            .into_response()
    } else {
        let conf_esc = html_escape(&confirmation_id);
        Html(format!(
            r#"<!DOCTYPE html><html><head><title>Order Confirmed</title></head><body><div data-testid="order-confirmation" role="status">Order confirmed: <span data-testid="confirmation-id">{conf_esc}</span></div></body></html>"#
        ))
        .into_response()
    }
}

// ============================================================================
// Cookie Persistence Fixtures & Formatters
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<i64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<String>,
}

/// Build standard X.com `auth_token` (`HttpOnly`) + `ct0` CSRF cookie pair.
pub fn x_auth_cookies(domain: &str, auth_token: &str, ct0: &str) -> Vec<FixtureCookie> {
    let future_exp = Utc::now().timestamp() + 86_400 * 30;
    vec![
        FixtureCookie {
            name: "auth_token".to_string(),
            value: auth_token.to_string(),
            domain: domain.to_string(),
            path: "/".to_string(),
            expires: Some(future_exp),
            http_only: true,
            secure: false,
            same_site: Some("Lax".to_string()),
        },
        FixtureCookie {
            name: "ct0".to_string(),
            value: ct0.to_string(),
            domain: domain.to_string(),
            path: "/".to_string(),
            expires: Some(future_exp),
            http_only: false,
            secure: false,
            same_site: Some("Lax".to_string()),
        },
    ]
}

/// Serialize cookies into standard 7-column Netscape HTTP Cookie File format.
pub fn generate_netscape_cookies(cookies: &[FixtureCookie]) -> String {
    let mut out = String::from(
        "# Netscape HTTP Cookie File\n# https://curl.se/docs/http-cookies.html\n",
    );
    for c in cookies {
        let include_subdomains = if c.domain.starts_with('.') {
            "TRUE"
        } else {
            "FALSE"
        };
        let domain_col = if c.http_only {
            format!("#HttpOnly_{}", c.domain)
        } else {
            c.domain.clone()
        };
        let secure = if c.secure { "TRUE" } else { "FALSE" };
        let expires = c.expires.unwrap_or(0);
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            domain_col, include_subdomains, c.path, secure, expires, c.name, c.value
        ));
    }
    out
}

/// Serialize cookies into EditThisCookie browser-extension JSON export format.
pub fn generate_edit_this_cookie_json(cookies: &[FixtureCookie]) -> String {
    let items: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            let same_site = match c.same_site.as_deref() {
                Some("Lax") | Some("lax") => "lax",
                Some("Strict") | Some("strict") => "strict",
                Some("None") | Some("none") | Some("no_restriction") => "no_restriction",
                _ => "unspecified",
            };
            let mut obj = json!({
                "name": c.name,
                "value": c.value,
                "domain": c.domain,
                "hostOnly": !c.domain.starts_with('.'),
                "path": c.path,
                "secure": c.secure,
                "httpOnly": c.http_only,
                "sameSite": same_site,
                "session": c.expires.is_none(),
            });
            if let Some(exp) = c.expires {
                obj["expirationDate"] = json!(exp as f64);
            }
            obj
        })
        .collect();
    serde_json::to_string_pretty(&items).expect("serialize EditThisCookie JSON")
}

/// Serialize cookies into CDP `Network.Cookie` / `bot-browser` `cookies.json` format.
pub fn generate_cdp_cookies_json(cookies: &[FixtureCookie]) -> String {
    let items: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            let same_site = match c.same_site.as_deref() {
                Some("Strict") | Some("strict") => "Strict",
                Some("None") | Some("none") | Some("no_restriction") => "None",
                _ => "Lax",
            };
            json!({
                "name": c.name,
                "value": c.value,
                "domain": c.domain,
                "path": c.path,
                "expires": c.expires.unwrap_or(-1),
                "httpOnly": c.http_only,
                "secure": c.secure,
                "sameSite": same_site,
                "session": c.expires.is_none(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).expect("serialize CDP cookies JSON")
}

/// Format a slice of `FixtureCookie`s into an HTTP `Cookie` request header value.
pub fn format_cookie_header(cookies: &[FixtureCookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Format a `HashMap<String, String>` into an HTTP `Cookie` request header value.
pub fn format_cookie_map_header(cookies: &HashMap<String, String>) -> String {
    let mut pairs: Vec<(&String, &String)> = cookies.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

// ============================================================================
// Synthetic Fake Secret Fixtures
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct FakeSecretFixtures {
    pub auth_token: &'static str,
    pub ct0: &'static str,
    pub bearer_token: &'static str,
    pub password: &'static str,
}

pub fn fake_secrets() -> FakeSecretFixtures {
    FakeSecretFixtures {
        auth_token: "mock_auth_token_0123456789abcdef_synthetic_fixture",
        ct0: "mock_ct0_fedcba9876543210_synthetic_fixture",
        bearer_token: "Bearer sk_test_synthetic_fixture_0000000000000000",
        password: "SyntheticTestPass!2026#Fixture",
    }
}

impl FakeSecretFixtures {
    pub fn all_values(&self) -> [(&'static str, &'static str); 4] {
        [
            ("auth_token", self.auth_token),
            ("ct0", self.ct0),
            ("bearer_token", self.bearer_token),
            ("password", self.password),
        ]
    }

    /// Return the label of the first synthetic secret found verbatim in `haystack`.
    pub fn contains_any_secret(&self, haystack: &str) -> Option<&'static str> {
        for (label, value) in self.all_values() {
            if haystack.contains(value) {
                return Some(label);
            }
        }
        if let Some(bare_bearer) = self.bearer_token.strip_prefix("Bearer ")
            && !bare_bearer.is_empty()
            && haystack.contains(bare_bearer)
        {
            return Some("bearer_token");
        }
        None
    }
}

// ============================================================================
// TempDir Isolation, Latency Budget & Non-Blocking Loop Harness Utilities
// ============================================================================

/// Create an isolated RAII `tempfile::TempDir` inside `std::env::temp_dir()`.
pub fn isolated_temp_dir(prefix: &str) -> anyhow::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(&format!("gpui_tg_browser_e2e_{prefix}_"))
        .tempdir()
        .context("create isolated browser e2e temp dir")
}

#[derive(Debug, Clone)]
pub struct StepLatencyMeasurement<T> {
    pub label: String,
    pub value: T,
    pub elapsed_ms: f64,
}

/// Measure wall-clock duration in milliseconds of an async browser/E2E step.
pub async fn measure_step_ms<F, Fut, T>(
    label: impl Into<String>,
    fut: F,
) -> StepLatencyMeasurement<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let start = Instant::now();
    let value = fut().await;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    StepLatencyMeasurement {
        label: label.into(),
        value,
        elapsed_ms,
    }
}

#[derive(Debug, Clone)]
pub struct LatencyBudget {
    pub max_step_ms: f64,
    pub max_total_ms: f64,
    pub samples: Vec<(String, f64)>,
}

impl LatencyBudget {
    pub fn new(max_step_ms: f64, max_total_ms: f64) -> Self {
        Self {
            max_step_ms,
            max_total_ms,
            samples: Vec::new(),
        }
    }

    pub fn record(&mut self, label: impl Into<String>, elapsed_ms: f64) {
        self.samples.push((label.into(), elapsed_ms));
    }

    pub async fn measure<F, Fut, T>(&mut self, label: impl Into<String>, fut: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let measurement = measure_step_ms(label, fut).await;
        self.record(measurement.label, measurement.elapsed_ms);
        measurement.value
    }

    pub fn total_ms(&self) -> f64 {
        self.samples.iter().map(|(_, ms)| *ms).sum()
    }

    pub fn max_observed_step_ms(&self) -> f64 {
        self.samples
            .iter()
            .map(|(_, ms)| *ms)
            .fold(0.0_f64, f64::max)
    }

    pub fn assert_within_budget(&self) -> anyhow::Result<()> {
        if !self.max_step_ms.is_finite() || self.max_step_ms < 0.0 {
            bail!("invalid max_step_ms budget: {}", self.max_step_ms);
        }
        if !self.max_total_ms.is_finite() || self.max_total_ms < 0.0 {
            bail!("invalid max_total_ms budget: {}", self.max_total_ms);
        }
        for (label, ms) in &self.samples {
            if !ms.is_finite() || *ms < 0.0 || !(*ms <= self.max_step_ms) {
                bail!(
                    "step '{label}' exceeded max_step_ms budget: {ms:.2}ms > {:.2}ms",
                    self.max_step_ms
                );
            }
        }
        let total = self.total_ms();
        if !total.is_finite() || !(total <= self.max_total_ms) {
            bail!(
                "total latency exceeded max_total_ms budget: {total:.2}ms > {:.2}ms",
                self.max_total_ms
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NonBlockingUiReport {
    pub ticks_completed: usize,
    pub max_tick_gap_ms: f64,
    pub background_elapsed_ms: f64,
}

/// Run `background_op` concurrently with a simulated UI loop ticker on the same async runtime
/// and assert that the UI loop never stalls beyond `max_allowed_tick_gap_ms`.
pub async fn assert_ui_loop_non_blocking<F, Fut, T>(
    background_op: F,
    tick_interval: Duration,
    max_allowed_tick_gap_ms: f64,
) -> anyhow::Result<(T, NonBlockingUiReport)>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    if !max_allowed_tick_gap_ms.is_finite() || max_allowed_tick_gap_ms < 0.0 {
        bail!(
            "invalid max_allowed_tick_gap_ms budget: {max_allowed_tick_gap_ms}"
        );
    }

    let op_fut = background_op();
    tokio::pin!(op_fut);

    let start = Instant::now();
    let mut last_tick = start;
    let mut max_tick_gap_ms = 0.0_f64;
    let mut ticks_completed = 0_usize;
    let mut ticker = tokio::time::interval(tick_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result = loop {
        tokio::select! {
            biased;
            res = &mut op_fut => {
                let gap_ms = last_tick.elapsed().as_secs_f64() * 1000.0;
                if !(gap_ms <= max_tick_gap_ms) {
                    max_tick_gap_ms = gap_ms;
                }
                break res;
            }
            _ = ticker.tick() => {
                let now = Instant::now();
                let gap_ms = now.duration_since(last_tick).as_secs_f64() * 1000.0;
                if !(gap_ms <= max_tick_gap_ms) {
                    max_tick_gap_ms = gap_ms;
                }
                last_tick = now;
                ticks_completed += 1;
            }
        }
    };

    let background_elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    if !(max_tick_gap_ms <= max_allowed_tick_gap_ms) {
        bail!(
            "UI loop tick gap stalled at {max_tick_gap_ms:.2}ms (budget {max_allowed_tick_gap_ms:.2}ms, ticks={ticks_completed})"
        );
    }

    Ok((
        result,
        NonBlockingUiReport {
            ticks_completed,
            max_tick_gap_ms,
            background_elapsed_ms,
        },
    ))
}

// ============================================================================
// Loopback OpenAI-Compatible Mock LLM Server
// ============================================================================

pub struct MockOpenAiServer {
    addr: SocketAddr,
    call_count: Arc<AtomicUsize>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl MockOpenAiServer {
    pub async fn start(responses: Vec<Value>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind ephemeral mock OpenAI listener")?;
        let addr = listener.local_addr().context("read mock OpenAI local_addr")?;
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_srv = Arc::clone(&call_count);
        let (tx, mut rx) = oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((mut sock, _)) = accepted else {
                            break;
                        };
                        let q = Arc::clone(&queue);
                        let cnt = Arc::clone(&call_count_srv);
                        tokio::spawn(async move {
                            let mut buf = Vec::with_capacity(8192);
                            let mut tmp = [0u8; 4096];
                            let mut content_len = 0usize;
                            let mut header_end = None;
                            loop {
                                let Ok(n) = sock.read(&mut tmp).await else {
                                    return;
                                };
                                if n == 0 {
                                    break;
                                }
                                buf.extend_from_slice(&tmp[..n]);
                                if header_end.is_none() {
                                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                        header_end = Some(pos + 4);
                                        let headers = String::from_utf8_lossy(&buf[..pos]);
                                        for line in headers.lines() {
                                            if let Some((k, v)) = line.split_once(':') {
                                                if k.trim().eq_ignore_ascii_case("content-length") {
                                                    content_len = v.trim().parse::<usize>().unwrap_or(0);
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Some(hend) = header_end {
                                    if buf.len() >= hend + content_len {
                                        break;
                                    }
                                }
                            }

                            cnt.fetch_add(1, Ordering::SeqCst);
                            let next_msg = q.lock().pop_front().unwrap_or_else(|| {
                                json!({
                                    "id": "chatcmpl-end",
                                    "choices": [{
                                        "message": { "role": "assistant", "content": "Completed." },
                                        "finish_reason": "stop"
                                    }],
                                    "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
                                })
                            });

                            let body = serde_json::to_vec(&next_msg).unwrap_or_default();
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = sock.write_all(resp.as_bytes()).await;
                            let _ = sock.write_all(&body).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            addr,
            call_count,
            shutdown_tx: Some(tx),
            task: Some(task),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn call_count_arc(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.call_count)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockOpenAiServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub fn mock_openai_tool_call(call_id: &str, name: &str, args: Value) -> Value {
    json!({
        "id": format!("chatcmpl-{call_id}"),
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string())
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 12, "total_tokens": 32 }
    })
}

pub fn mock_openai_text(text: &str) -> Value {
    json!({
        "id": "chatcmpl-final",
        "choices": [{
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 15, "completion_tokens": 8, "total_tokens": 23 }
    })
}
