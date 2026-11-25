use std::time::Duration;

use e2e_suite::browser_e2e_fixtures::*;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, COOKIE, LOCATION};
use serde_json::json;

#[tokio::test]
async fn mock_x_login_form_and_auth_flow() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let client = no_proxy_http_client();

    let login_page = client
        .get(server.url("/login"))
        .send()
        .await
        .expect("GET /login");
    assert_eq!(login_page.status(), StatusCode::OK);
    let html = login_page.text().await.expect("read /login html");
    assert!(html.contains(r#"data-testid="ocfEnterTextTextInput""#));
    assert!(html.contains(r#"data-testid="LoginForm_Password_Input""#));
    assert!(html.contains(r#"data-testid="LoginForm_Login_Button""#));
    assert!(html.contains(r#"role="form""#));
    assert!(html.contains(r#"aria-label="Sign in to X""#));

    let empty_resp = client
        .post(server.url("/login"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /login empty");
    assert_eq!(empty_resp.status(), StatusCode::BAD_REQUEST);

    let missing_pw = client
        .post(server.url("/login"))
        .json(&json!({ "username": DEFAULT_X_USERNAME }))
        .send()
        .await
        .expect("POST /login missing pw");
    assert_eq!(missing_pw.status(), StatusCode::BAD_REQUEST);

    let wrong_pw = client
        .post(server.url("/login"))
        .json(&json!({ "username": DEFAULT_X_USERNAME, "password": "wrong-password" }))
        .send()
        .await
        .expect("POST /login wrong pw");
    assert_eq!(wrong_pw.status(), StatusCode::UNAUTHORIZED);

    let ok_json = client
        .post(server.url("/login"))
        .json(&json!({ "username": DEFAULT_X_USERNAME, "password": DEFAULT_X_PASSWORD }))
        .send()
        .await
        .expect("POST /login valid json");
    assert_eq!(ok_json.status(), StatusCode::OK);
    let set_cookies = parse_set_cookie_headers(ok_json.headers());
    assert!(set_cookies.contains_key("auth_token"));
    assert!(set_cookies.contains_key("ct0"));
    let body: serde_json::Value = ok_json.json().await.expect("login json body");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["auth_token"], json!(set_cookies["auth_token"]));
    assert_eq!(body["ct0"], json!(set_cookies["ct0"]));

    let form_resp = client
        .post(server.url("/login"))
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "username={}&password={}",
            DEFAULT_X_USERNAME, DEFAULT_X_PASSWORD
        ))
        .send()
        .await
        .expect("POST /login form");
    assert_eq!(form_resp.status(), StatusCode::OK);
    let form_cookies = parse_set_cookie_headers(form_resp.headers());
    assert!(form_cookies.contains_key("auth_token"));
    assert!(form_cookies.contains_key("ct0"));

    server.shutdown().await;
}

#[tokio::test]
async fn mock_x_cookie_gating_home_dom_and_challenge() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let no_redir = no_redirect_http_client();
    let client = no_proxy_http_client();

    let home_unauth = no_redir
        .get(server.url("/home"))
        .send()
        .await
        .expect("GET /home unauthenticated");
    assert_eq!(home_unauth.status(), StatusCode::FOUND);
    assert_eq!(
        home_unauth
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );

    let home_unauth_json = client
        .get(server.url("/home"))
        .header(ACCEPT, "application/json")
        .send()
        .await
        .expect("GET /home Accept json");
    assert_eq!(home_unauth_json.status(), StatusCode::UNAUTHORIZED);

    for (method, path) in [
        ("GET", "/api/me"),
        ("GET", "/api/tweets"),
        ("POST", "/api/tweets"),
        ("POST", "/api/tweets/1001/like"),
        ("POST", "/api/tweets/1001/repost"),
    ] {
        let req = match method {
            "GET" => client.get(server.url(path)),
            _ => client.post(server.url(path)).json(&json!({ "text": "x" })),
        };
        let resp = req.send().await.expect("unauthenticated api request");
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "expected 401 for {method} {path}"
        );
    }

    let login_resp = client
        .post(server.url("/login"))
        .json(&json!({ "username": DEFAULT_X_USERNAME, "password": DEFAULT_X_PASSWORD }))
        .send()
        .await
        .expect("login");
    let cookies = parse_set_cookie_headers(login_resp.headers());
    let auth_token = cookies.get("auth_token").unwrap();

    let bad_ct0 = client
        .get(server.url("/api/me"))
        .header(COOKIE, format!("auth_token={auth_token}; ct0=wrong_ct0"))
        .send()
        .await
        .expect("GET /api/me mismatched ct0");
    assert_eq!(bad_ct0.status(), StatusCode::UNAUTHORIZED);

    let cookie_hdr = format_cookie_map_header(&cookies);
    let home_ok = client
        .get(server.url("/home"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /home authenticated");
    assert_eq!(home_ok.status(), StatusCode::OK);
    let home_html = home_ok.text().await.expect("read /home html");
    assert!(home_html.contains(r#"data-testid="tweetTextarea_0""#));
    assert!(home_html.contains(r#"data-testid="tweetButtonInline""#));
    assert!(home_html.contains(r#"data-testid="tweet""#));
    assert!(home_html.contains(r#"data-testid="like""#));
    assert!(home_html.contains(r#"data-testid="repost""#));
    assert!(home_html.contains(r#"role="article""#));
    assert!(home_html.contains("aria-label="));

    let challenge_resp = client
        .get(server.url("/challenge"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /challenge");
    assert_eq!(challenge_resp.status(), StatusCode::OK);
    let challenge_html = challenge_resp.text().await.expect("read /challenge html");
    assert!(challenge_html.contains(r#"data-testid="human-captcha-checkbox""#));

    let challenge_verify = client
        .post(server.url("/challenge"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "human_verified": true }))
        .send()
        .await
        .expect("POST /challenge verify");
    assert_eq!(challenge_verify.status(), StatusCode::OK);
    assert!(
        server
            .state()
            .challenge_verified_tokens
            .read()
            .contains(auth_token)
    );

    server.shutdown().await;
}

#[tokio::test]
async fn mock_x_tweet_create_like_and_repost_mutations() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let client = no_proxy_http_client();

    let login_resp = client
        .post(server.url("/login"))
        .json(&json!({ "username": DEFAULT_X_USERNAME, "password": DEFAULT_X_PASSWORD }))
        .send()
        .await
        .expect("login");
    let cookies = parse_set_cookie_headers(login_resp.headers());
    let cookie_hdr = format_cookie_map_header(&cookies);

    let me: serde_json::Value = client
        .get(server.url("/api/me"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /api/me")
        .json()
        .await
        .expect("parse /api/me");
    assert_eq!(me["handle"], json!(DEFAULT_X_HANDLE));

    let initial_tweets: serde_json::Value = client
        .get(server.url("/api/tweets"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /api/tweets")
        .json()
        .await
        .expect("parse /api/tweets");
    assert!(initial_tweets["tweets"].as_array().unwrap().len() >= 2);

    let empty_tweet = client
        .post(server.url("/api/tweets"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "text": "   " }))
        .send()
        .await
        .expect("POST /api/tweets blank");
    assert_eq!(empty_tweet.status(), StatusCode::BAD_REQUEST);

    let created = client
        .post(server.url("/api/tweets"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "text": "E2E automated post" }))
        .send()
        .await
        .expect("POST /api/tweets");
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_json: serde_json::Value = created.json().await.expect("created tweet json");
    assert_eq!(created_json["tweet"]["text"], json!("E2E automated post"));
    assert_eq!(server.tweet_count(), 3);

    let home_html = client
        .get(server.url("/home"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /home")
        .text()
        .await
        .expect("home text");
    assert!(home_html.contains("E2E automated post"));

    let like_on: serde_json::Value = client
        .post(server.url("/api/tweets/1001/like"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("like tweet")
        .json()
        .await
        .expect("like json");
    assert_eq!(like_on["likes"], json!(1));
    assert_eq!(like_on["liked"], json!(true));

    let like_off: serde_json::Value = client
        .post(server.url("/api/tweets/1001/like"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("unlike tweet")
        .json()
        .await
        .expect("unlike json");
    assert_eq!(like_off["likes"], json!(0));
    assert_eq!(like_off["liked"], json!(false));

    let repost_on: serde_json::Value = client
        .post(server.url("/api/tweets/1001/repost"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("repost tweet")
        .json()
        .await
        .expect("repost json");
    assert_eq!(repost_on["reposts"], json!(1));
    assert_eq!(repost_on["reposted"], json!(true));

    let repost_off: serde_json::Value = client
        .post(server.url("/api/tweets/1001/repost"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("unrepost tweet")
        .json()
        .await
        .expect("unrepost json");
    assert_eq!(repost_off["reposts"], json!(0));
    assert_eq!(repost_off["reposted"], json!(false));

    let missing_like = client
        .post(server.url("/api/tweets/nonexistent/like"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("like nonexistent");
    assert_eq!(missing_like.status(), StatusCode::NOT_FOUND);

    let missing_repost = client
        .post(server.url("/api/tweets/nonexistent/repost"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("repost nonexistent");
    assert_eq!(missing_repost.status(), StatusCode::NOT_FOUND);

    server.shutdown().await;
}

#[tokio::test]
async fn mock_x_seeded_session_and_graceful_shutdown() {
    let server = MockXServer::start().await.expect("start MockXServer");
    let secrets = fake_secrets();
    let seeded = server.seed_session(secrets.auth_token, secrets.ct0);
    let client = no_proxy_http_client();

    let cookie_hdr = format!("auth_token={}; ct0={}", seeded.auth_token, seeded.ct0);
    let me = client
        .get(server.url("/api/me"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /api/me with seeded session");
    assert_eq!(me.status(), StatusCode::OK);

    let home = client
        .get(server.url("/home"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /home with seeded session");
    assert_eq!(home.status(), StatusCode::OK);

    let login_url = server.url("/login");
    server.shutdown().await;

    let after_shutdown = client.get(login_url).send().await;
    assert!(
        after_shutdown.is_err(),
        "requests after graceful shutdown must fail"
    );
}

#[tokio::test]
async fn mock_webapp_search_filter_and_pagination() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let client = no_proxy_http_client();

    let index = client
        .get(server.url("/"))
        .send()
        .await
        .expect("GET /");
    assert_eq!(index.status(), StatusCode::OK);
    let index_cookies = parse_set_cookie_headers(index.headers());
    assert!(index_cookies.contains_key("shop_sid"));
    let index_html = index.text().await.expect("read / html");
    assert!(index_html.contains(r#"data-testid="search-input""#));
    assert!(index_html.contains(r#"data-testid="category-select""#));
    assert!(index_html.contains(r#"data-testid="search-submit""#));
    assert!(index_html.contains(r#"role="search""#));

    let page1 = client
        .get(server.url("/search?page=1&page_size=5"))
        .send()
        .await
        .expect("GET /search page 1");
    assert_eq!(page1.status(), StatusCode::OK);
    let page1_html = page1.text().await.expect("read page 1 html");
    assert!(page1_html.contains(r#"data-testid="results-table""#));
    assert_eq!(
        page1_html.matches(r#"data-testid="result-row""#).count(),
        5
    );
    assert!(page1_html.contains("Page 1 of 3"));
    assert!(page1_html.contains(r#"data-testid="next-page""#));
    assert!(!page1_html.contains(r#"data-testid="prev-page""#));

    let page2 = client
        .get(server.url("/search?page=2&page_size=5"))
        .send()
        .await
        .expect("GET /search page 2");
    assert_eq!(page2.status(), StatusCode::OK);
    let page2_html = page2.text().await.expect("read page 2 html");
    assert_eq!(
        page2_html.matches(r#"data-testid="result-row""#).count(),
        5
    );
    assert!(page2_html.contains("Page 2 of 3"));
    assert!(page2_html.contains(r#"data-testid="prev-page""#));
    assert!(page2_html.contains(r#"data-testid="next-page""#));

    let filtered_json: serde_json::Value = client
        .get(server.url("/search?category=books&q=Rust"))
        .header(ACCEPT, "application/json")
        .send()
        .await
        .expect("GET /search filtered json")
        .json()
        .await
        .expect("parse filtered json");
    let items = filtered_json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(item["category"], json!("books"));
        assert!(
            item["title"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("rust")
        );
    }

    let format_json: serde_json::Value = client
        .get(server.url("/search?category=research&format=json"))
        .send()
        .await
        .expect("GET /search format=json")
        .json()
        .await
        .expect("parse format=json");
    assert_eq!(format_json["total_items"], json!(2));

    let clamped_page_html = client
        .get(server.url("/search?page=99&page_size=5"))
        .send()
        .await
        .expect("GET /search out-of-range page")
        .text()
        .await
        .expect("read clamped page html");
    assert!(clamped_page_html.contains("Page 3 of 3"));
    assert!(clamped_page_html.contains("page=2&amp;page_size=5"));
    assert!(!clamped_page_html.contains("page=98"));
    assert!(!clamped_page_html.contains(r#"data-testid="next-page""#));

    let special_q_page1 = client
        .get(server.url("/search?q=Rust%20%26%20Browser%231&category=all&page=1&page_size=1"))
        .send()
        .await
        .expect("GET /search special query page 1")
        .text()
        .await
        .expect("read special query page 1 html");
    assert!(special_q_page1.contains("Page 1 of 2"));
    let expected_next_href_attr =
        r#"href="/search?q=Rust%20%26%20Browser%231&amp;category=all&amp;page=2&amp;page_size=1""#;
    assert!(
        special_q_page1.contains(expected_next_href_attr),
        "expected percent-encoded next-page href in HTML, got:\n{special_q_page1}"
    );

    let href_start = special_q_page1
        .find(r#"href="/search?q=Rust%20%26%20Browser%231"#)
        .expect("locate next-page href")
        + 6;
    let href_end = special_q_page1[href_start..]
        .find('"')
        .expect("close href quote")
        + href_start;
    let followed_path = special_q_page1[href_start..href_end].replace("&amp;", "&");
    assert_eq!(
        followed_path,
        "/search?q=Rust%20%26%20Browser%231&category=all&page=2&page_size=1"
    );

    let followed_html = client
        .get(server.url(&followed_path))
        .send()
        .await
        .expect("follow encoded next-page href")
        .text()
        .await
        .expect("read followed page 2 html");
    assert!(followed_html.contains("Page 2 of 2"));
    assert!(followed_html.contains(
        r#"href="/search?q=Rust%20%26%20Browser%231&amp;category=all&amp;page=1&amp;page_size=1""#
    ));

    let followed_json: serde_json::Value = client
        .get(server.url(&followed_path))
        .header(ACCEPT, "application/json")
        .send()
        .await
        .expect("follow encoded next-page href as json")
        .json()
        .await
        .expect("parse followed json");
    assert_eq!(followed_json["total_items"], json!(2));
    assert_eq!(followed_json["page"], json!(2));
    assert_eq!(followed_json["items"].as_array().unwrap().len(), 1);
    assert_eq!(followed_json["items"][0]["id"], json!("item-08"));

    server.shutdown().await;
}

#[tokio::test]
async fn mock_webapp_item_cart_and_checkout_validation() {
    let server = MockWebAppServer::start()
        .await
        .expect("start MockWebAppServer");
    let client = no_proxy_http_client();

    let missing_item = client
        .get(server.url("/item/missing-999"))
        .send()
        .await
        .expect("GET missing item");
    assert_eq!(missing_item.status(), StatusCode::NOT_FOUND);

    let item_ok = client
        .get(server.url("/item/item-01"))
        .send()
        .await
        .expect("GET /item/item-01");
    assert_eq!(item_ok.status(), StatusCode::OK);
    let item_html = item_ok.text().await.expect("read item html");
    assert!(item_html.contains(r#"data-testid="item-title""#));
    assert!(item_html.contains(r#"data-testid="qty-input""#));
    assert!(item_html.contains(r#"data-testid="add-to-cart-btn""#));

    let unknown_add = client
        .post(server.url("/cart/add"))
        .json(&json!({ "item_id": "missing-999", "quantity": 1 }))
        .send()
        .await
        .expect("POST /cart/add unknown item");
    assert_eq!(unknown_add.status(), StatusCode::NOT_FOUND);

    let zero_qty = client
        .post(server.url("/cart/add"))
        .json(&json!({ "item_id": "item-01", "quantity": 0 }))
        .send()
        .await
        .expect("POST /cart/add qty 0");
    assert_eq!(zero_qty.status(), StatusCode::BAD_REQUEST);

    let empty_checkout = client
        .post(server.url("/checkout"))
        .json(&json!({ "name": " ", "email": "bad" }))
        .send()
        .await
        .expect("POST /checkout empty cart");
    assert_eq!(empty_checkout.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let err_json: serde_json::Value = empty_checkout.json().await.expect("422 json");
    let fields: Vec<&str> = err_json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["field"].as_str())
        .collect();
    assert!(fields.contains(&"cart"));
    assert!(fields.contains(&"name"));
    assert!(fields.contains(&"email"));

    let add_ok = client
        .post(server.url("/cart/add"))
        .json(&json!({ "item_id": "item-01", "quantity": 2 }))
        .send()
        .await
        .expect("POST /cart/add valid");
    assert_eq!(add_ok.status(), StatusCode::OK);
    let cart_cookies = parse_set_cookie_headers(add_ok.headers());
    let sid = cart_cookies.get("shop_sid").expect("shop_sid cookie");
    let cookie_hdr = format!("shop_sid={sid}");

    let checkout_page = client
        .get(server.url("/checkout"))
        .header(COOKIE, &cookie_hdr)
        .send()
        .await
        .expect("GET /checkout");
    assert_eq!(checkout_page.status(), StatusCode::OK);
    let checkout_html = checkout_page.text().await.expect("checkout html");
    assert!(checkout_html.contains(r#"<form id="checkout-form""#));
    assert!(checkout_html.contains("novalidate"));
    assert!(checkout_html.contains(r#"<span data-testid="cart-item-count">2</span>"#));
    assert!(checkout_html.contains(r#"<span data-testid="cart-total-cents">9000</span>"#));

    let bad_email = client
        .post(server.url("/checkout"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "name": "Alice", "email": "not-an-email" }))
        .send()
        .await
        .expect("POST /checkout bad email");
    assert_eq!(bad_email.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let valid_checkout = client
        .post(server.url("/checkout"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "name": "Alice", "email": "alice@example.test" }))
        .send()
        .await
        .expect("POST /checkout valid");
    assert_eq!(valid_checkout.status(), StatusCode::OK);
    let order_json: serde_json::Value = valid_checkout.json().await.expect("order json");
    assert_eq!(order_json["ok"], json!(true));
    assert!(
        order_json["confirmation_id"]
            .as_str()
            .unwrap()
            .starts_with("ORD-")
    );
    assert_eq!(order_json["total_cents"], json!(9000));
    assert_eq!(order_json["item_count"], json!(2));
    assert_eq!(server.order_count(), 1);

    let repeat_checkout = client
        .post(server.url("/checkout"))
        .header(COOKIE, &cookie_hdr)
        .json(&json!({ "name": "Alice", "email": "alice@example.test" }))
        .send()
        .await
        .expect("repeat POST /checkout after cart drained");
    assert_eq!(repeat_checkout.status(), StatusCode::UNPROCESSABLE_ENTITY);

    server.shutdown().await;
}

#[tokio::test]
async fn cookie_generators_and_secret_fixtures_self_test() {
    let secrets = fake_secrets();
    let cookies = x_auth_cookies("127.0.0.1", secrets.auth_token, secrets.ct0);
    assert_eq!(cookies.len(), 2);

    let netscape = generate_netscape_cookies(&cookies);
    assert!(netscape.starts_with("# Netscape HTTP Cookie File"));
    let data_lines: Vec<&str> = netscape
        .lines()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty()
                && (trimmed.starts_with("#HttpOnly_") || !trimmed.starts_with('#'))
        })
        .collect();
    assert_eq!(data_lines.len(), 2);
    assert!(
        data_lines[0].starts_with("#HttpOnly_127.0.0.1\tFALSE\t/\tFALSE\t"),
        "http_only cookie must carry #HttpOnly_ prefix, got: {}",
        data_lines[0]
    );
    assert!(
        !data_lines[1].starts_with("#HttpOnly_")
            && data_lines[1].starts_with("127.0.0.1\tFALSE\t/\tFALSE\t"),
        "non-http_only cookie must not carry #HttpOnly_ prefix, got: {}",
        data_lines[1]
    );
    for line in &data_lines {
        let cols_str = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        assert_eq!(cols_str.split('\t').count(), 7);
    }
    assert!(netscape.contains(secrets.auth_token));
    assert!(netscape.contains(secrets.ct0));

    let etc_json: serde_json::Value =
        serde_json::from_str(&generate_edit_this_cookie_json(&cookies))
            .expect("parse EditThisCookie JSON");
    let etc_arr = etc_json.as_array().unwrap();
    assert_eq!(etc_arr.len(), 2);
    assert_eq!(etc_arr[0]["httpOnly"], json!(true));
    assert_eq!(etc_arr[0]["sameSite"], json!("lax"));
    assert!(etc_arr[0]["expirationDate"].is_number());

    let cdp_json: serde_json::Value =
        serde_json::from_str(&generate_cdp_cookies_json(&cookies)).expect("parse CDP JSON");
    let cdp_arr = cdp_json.as_array().unwrap();
    assert_eq!(cdp_arr.len(), 2);
    assert_eq!(cdp_arr[0]["httpOnly"], json!(true));
    assert_eq!(cdp_arr[0]["sameSite"], json!("Lax"));
    assert!(cdp_arr[0]["expires"].is_i64());

    let header = format_cookie_header(&cookies);
    assert!(header.contains("auth_token="));
    assert!(header.contains("ct0="));

    for (label, val) in secrets.all_values() {
        let payload = format!("leaked: {val}");
        assert_eq!(secrets.contains_any_secret(&payload), Some(label));
    }
    let bare_bearer = secrets
        .bearer_token
        .strip_prefix("Bearer ")
        .expect("bearer_token starts with Bearer ");
    assert_eq!(
        secrets.contains_any_secret(&format!("Authorization={bare_bearer}")),
        Some("bearer_token")
    );
    assert_eq!(
        secrets.contains_any_secret(r#"{"steps":[{"locator":{"test_id":"submit"}}]}"#),
        None
    );
}

#[tokio::test]
async fn temp_dir_isolation_latency_budget_and_non_blocking_ui_self_test() {
    let dir_a = isolated_temp_dir("self_test_a").expect("create dir_a");
    let dir_b = isolated_temp_dir("self_test_b").expect("create dir_b");
    assert_ne!(dir_a.path(), dir_b.path());
    assert!(dir_a.path().starts_with(std::env::temp_dir()));
    assert!(dir_b.path().starts_with(std::env::temp_dir()));

    let marker = dir_a.path().join("marker.txt");
    std::fs::write(&marker, b"ok").expect("write marker");
    assert!(marker.exists());
    let path_a = dir_a.path().to_path_buf();
    drop(dir_a);
    assert!(!path_a.exists(), "TempDir must clean up on Drop");
    drop(dir_b);

    let server = MockXServer::start().await.expect("start MockXServer");
    let client = no_proxy_http_client();

    let mut budget = LatencyBudget::new(2_000.0, 5_000.0);
    let status = budget
        .measure("get_login", || async {
            client
                .get(server.url("/login"))
                .send()
                .await
                .expect("GET /login")
                .status()
        })
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(budget.max_observed_step_ms() >= 0.0);
    budget
        .assert_within_budget()
        .expect("loopback GET /login within generous latency budget");

    let mut tight_budget = LatencyBudget::new(0.01, 0.01);
    tight_budget
        .measure("sleep_5ms", || async {
            tokio::time::sleep(Duration::from_millis(5)).await;
        })
        .await;
    assert!(tight_budget.assert_within_budget().is_err());

    let (non_blocking_val, ui_report) = assert_ui_loop_non_blocking(
        || async {
            tokio::time::sleep(Duration::from_millis(45)).await;
            42_u32
        },
        Duration::from_millis(5),
        40.0,
    )
    .await
    .expect("async sleep must not block UI ticker");
    assert_eq!(non_blocking_val, 42);
    assert!(ui_report.ticks_completed >= 3);
    assert!(ui_report.background_elapsed_ms >= 40.0);

    let blocking_res = assert_ui_loop_non_blocking(
        || async {
            std::thread::sleep(Duration::from_millis(25));
            tokio::task::yield_now().await;
        },
        Duration::from_millis(5),
        10.0,
    )
    .await;
    assert!(
        blocking_res.is_err(),
        "synchronous thread sleep must trigger non-blocking UI loop failure"
    );

    let pre_await_blocking_res = assert_ui_loop_non_blocking(
        || async {
            std::thread::sleep(Duration::from_millis(50));
            tokio::time::sleep(Duration::from_millis(20)).await;
        },
        Duration::from_millis(5),
        10.0,
    )
    .await;
    assert!(
        pre_await_blocking_res.is_err(),
        "sync blocking before first .await must trigger non-blocking UI loop failure on tick 0"
    );

    let mut nan_sample_budget = LatencyBudget::new(100.0, 100.0);
    nan_sample_budget.record("nan_step", f64::NAN);
    assert!(
        nan_sample_budget.assert_within_budget().is_err(),
        "NaN sample must fail LatencyBudget::assert_within_budget"
    );

    let nan_limit_budget = LatencyBudget::new(f64::NAN, 100.0);
    assert!(
        nan_limit_budget.assert_within_budget().is_err(),
        "NaN max_step_ms must fail LatencyBudget::assert_within_budget"
    );

    let nan_ui_budget_res = assert_ui_loop_non_blocking(
        || async {
            tokio::time::sleep(Duration::from_millis(5)).await;
        },
        Duration::from_millis(2),
        f64::NAN,
    )
    .await;
    assert!(
        nan_ui_budget_res.is_err(),
        "NaN max_allowed_tick_gap_ms must fail assert_ui_loop_non_blocking"
    );

    server.shutdown().await;
}
