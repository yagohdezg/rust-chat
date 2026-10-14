mod common;

use base64::Engine as _;
use common::{Backend, Session, TestEnvBuilder};
use serde_json::json;

#[tokio::test]
async fn health_endpoint_responds() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let resp = env
        .http
        .get(format!("{}/health", env.base_url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn first_registered_user_is_admin() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let first = Session::register(&env.base_url, "admin@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(first.role, "admin");

    let second = Session::register(&env.base_url, "user@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(second.role, "user");
}

#[tokio::test]
async fn login_round_trip() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let registered = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();
    let logged_in = Session::login(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(registered.user_id, logged_in.user_id);
}

#[tokio::test]
async fn protected_routes_require_auth() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let resp = env
        .http
        .get(format!("{}/api/conversations", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn conversation_lifecycle() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let id = session.create_conversation("My chat").await.unwrap();
    let list = session.get_expect("/api/conversations").await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["title"], "My chat");

    let messages = session
        .get_expect(&format!("/api/conversations/{id}/messages"))
        .await
        .unwrap();
    assert!(messages.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn conversation_rename_archive_and_updated_at() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();
    let id = session.create_conversation("Original").await.unwrap();

    // Rename is reflected in the listing.
    let renamed = session
        .patch_expect(
            &format!("/api/conversations/{id}"),
            &json!({"title": "Renamed"}),
        )
        .await
        .unwrap();
    assert_eq!(renamed["title"], "Renamed");

    // Archiving hides the chat from the default listing but keeps it in the
    // archived view.
    session
        .patch_expect(
            &format!("/api/conversations/{id}"),
            &json!({"archived": true}),
        )
        .await
        .unwrap();
    let active = session.get_expect("/api/conversations").await.unwrap();
    assert!(
        active.as_array().unwrap().is_empty(),
        "archived chats leave the default list"
    );
    let archived = session
        .get_expect("/api/conversations?archived=true")
        .await
        .unwrap();
    assert_eq!(archived.as_array().unwrap().len(), 1);
    assert_eq!(archived[0]["archived"], json!(true));

    // Unarchiving restores it to the default listing.
    session
        .patch_expect(
            &format!("/api/conversations/{id}"),
            &json!({"archived": false}),
        )
        .await
        .unwrap();
    let active = session.get_expect("/api/conversations").await.unwrap();
    assert_eq!(active.as_array().unwrap().len(), 1);

    // A new message bumps `updated_at` so the chat sorts by its latest turn.
    let before = active[0]["updated_at"].as_str().unwrap().to_string();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    session
        .chat_sse(id, "hi", None, Some(provider_id))
        .await
        .unwrap();
    let after = session.get_expect("/api/conversations").await.unwrap();
    assert_ne!(
        after[0]["updated_at"].as_str().unwrap(),
        before,
        "updated_at should move after a new message"
    );
}

#[tokio::test]
async fn message_pagination_is_stable() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let id = session.create_conversation("paged").await.unwrap();

    for text in ["one", "two", "three"] {
        session
            .chat_sse(id, text, None, Some(provider_id))
            .await
            .unwrap();
    }

    let all = session
        .get_expect(&format!("/api/conversations/{id}/messages?limit=200"))
        .await
        .unwrap();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 6, "three turns, two messages each");
    assert_eq!(all[0]["role"], json!("user"));
    assert_eq!(all[0]["content"], json!("one"));

    // Walk backwards two at a time and rebuild the conversation from the pages.
    let mut pages: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(cursor) => format!("/api/conversations/{id}/messages?limit=2&before={cursor}"),
            None => format!("/api/conversations/{id}/messages?limit=2"),
        };
        let page = session.get_expect(&path).await.unwrap();
        let page = page.as_array().unwrap().clone();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 2, "page respects the requested size");
        cursor = Some(page[0]["id"].as_str().unwrap().to_string());
        pages.push(page);
    }
    assert_eq!(pages.len(), 3, "six messages in pages of two");

    let rebuilt: Vec<serde_json::Value> = pages.into_iter().rev().flatten().collect();
    assert_eq!(
        serde_json::Value::Array(rebuilt),
        serde_json::Value::Array(all.to_vec()),
        "walking the cursor reconstructs the conversation exactly"
    );

    // `after` returns the page immediately following a cursor.
    let after = session
        .get_expect(&format!(
            "/api/conversations/{id}/messages?limit=2&after={}",
            all[1]["id"].as_str().unwrap()
        ))
        .await
        .unwrap();
    assert_eq!(after.as_array().unwrap()[0]["content"], json!("two"));

    // `before` and `after` are mutually exclusive.
    let resp = session
        .get(&format!(
            "/api/conversations/{id}/messages?before={}&after={}",
            all[0]["id"].as_str().unwrap(),
            all[1]["id"].as_str().unwrap()
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn provider_lifecycle() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();

    let list = session.get_expect("/api/providers").await.unwrap();
    let providers = list.as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["id"], json!(provider_id));
    assert_eq!(providers[0]["has_key"], json!(true));

    let models = session
        .get_expect(&format!("/api/providers/{provider_id}/models"))
        .await
        .unwrap();
    assert!(models.as_array().is_some() || models["data"].is_array());
}

#[tokio::test]
async fn conversations_are_isolated_between_users() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let alice = Session::register(&env.base_url, "alice@example.com", "correct-horse-battery")
        .await
        .unwrap();
    let bob = Session::register(&env.base_url, "bob@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let id = alice.create_conversation("private").await.unwrap();
    let resp = bob
        .get(&format!("/api/conversations/{id}/messages"))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn conversations_are_isolated_between_users_runs_on_postgres() {
    if !common::postgres_available() {
        eprintln!("skipping: no container runtime available");
        return;
    }
    let env = TestEnvBuilder::new()
        .backend(Backend::Postgres)
        .start()
        .await
        .unwrap();

    let admin = Session::register(&env.base_url, "admin@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(admin.role, "admin");
    let user = Session::register(&env.base_url, "user@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(user.role, "user");

    let logged_in = Session::login(&env.base_url, "admin@example.com", "correct-horse-battery")
        .await
        .unwrap();
    assert_eq!(admin.user_id, logged_in.user_id);

    let provider_id = admin
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let providers = admin.get_expect("/api/providers").await.unwrap();
    assert_eq!(providers.as_array().unwrap().len(), 1);
    assert_eq!(providers[0]["id"], json!(provider_id));

    let id = admin.create_conversation("pg chat").await.unwrap();
    let list = admin.get_expect("/api/conversations").await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["title"], "pg chat");
    let messages = admin
        .get_expect(&format!("/api/conversations/{id}/messages"))
        .await
        .unwrap();
    assert!(messages.as_array().unwrap().is_empty());

    let resp = env
        .http
        .get(format!("{}/health", env.base_url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn upload_enforces_the_configured_size_limit() {
    let env = TestEnvBuilder::new()
        .env("UPLOAD_MAX_BYTES", "16")
        .start()
        .await
        .unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let payload = base64::engine::general_purpose::STANDARD.encode(vec![b'a'; 64]);
    let resp = session
        .post(
            "/api/files",
            &json!({"filename": "big.txt", "content_b64": payload}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let ok = base64::engine::general_purpose::STANDARD.encode(b"tiny");
    let resp = session
        .post(
            "/api/files",
            &json!({"filename": "tiny.txt", "content_b64": ok}),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn upload_sniffs_the_mime_type_from_bytes() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let png =
        base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR");
    session
        .post_expect(
            "/api/files",
            &json!({"filename": "photo.bin", "content_b64": png, "mime": "text/plain"}),
        )
        .await
        .unwrap();

    let files = session.get_expect("/api/files").await.unwrap();
    assert_eq!(files.as_array().unwrap().len(), 1);
    assert_eq!(
        files[0]["mime"],
        json!("image/png"),
        "the client-supplied MIME must not be trusted"
    );
}

#[tokio::test]
async fn upload_sanitizes_path_traversal_filenames() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let payload = base64::engine::general_purpose::STANDARD.encode(b"secret");
    let created = session
        .post_expect(
            "/api/files",
            &json!({"filename": "../../etc/passwd", "content_b64": payload}),
        )
        .await
        .unwrap();
    let filename = created["file"]["filename"].as_str().unwrap();
    assert!(
        !filename.contains('/'),
        "filename kept a path separator: {filename}"
    );
    assert!(
        !filename.starts_with('.'),
        "filename kept a leading dot: {filename}"
    );

    // The file is still downloadable under its sanitized name.
    let id = created["file"]["id"].as_str().unwrap();
    let resp = session.get(&format!("/api/files/{id}")).await.unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn upload_runs_the_scanner_hook() {
    let env = TestEnvBuilder::new()
        .env("UPLOAD_SCAN_COMMAND", "grep -q EICAR && exit 1 || exit 0")
        .start()
        .await
        .unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let clean = base64::engine::general_purpose::STANDARD.encode(b"hello");
    let resp = session
        .post(
            "/api/files",
            &json!({"filename": "clean.txt", "content_b64": clean}),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "clean upload should pass the hook"
    );

    let infected = base64::engine::general_purpose::STANDARD.encode(b"X5O!EICAR");
    let resp = session
        .post(
            "/api/files",
            &json!({"filename": "bad.txt", "content_b64": infected}),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "the scanner hook should reject a flagged upload"
    );
}

#[tokio::test]
async fn api_key_lifecycle() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let created = session
        .post_expect("/api/api-keys", &json!({"name": "ci"}))
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("rck_"), "unexpected key format: {key}");
    let id = created["api_key"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["api_key"]["name"], json!("ci"));
    assert!(
        created["api_key"]["key_hash"].is_null(),
        "the stored hash must not be serialized"
    );

    // The key authenticates the same routes as a bearer token.
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/api/me", session.base))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let listed = session.get_expect("/api/api-keys").await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0]["key_hash"].is_null());

    let resp = http
        .delete(format!("{}/api/api-keys/{id}", session.base))
        .bearer_auth(&session.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    // A revoked key is rejected.
    let resp = http
        .get(format!("{}/api/me", session.base))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Both lifecycle events are auditable.
    let audit = session
        .get_expect("/api/admin/audit?limit=50")
        .await
        .unwrap();
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["action"].as_str())
        .collect();
    assert!(actions.contains(&"api_key.create"), "audit: {actions:?}");
    assert!(actions.contains(&"api_key.revoke"), "audit: {actions:?}");
}

#[tokio::test]
async fn api_keys_are_scoped() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    let created = session
        .post_expect(
            "/api/api-keys",
            &json!({"name": "conversations only", "scopes": ["conversations"]}),
        )
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap().to_string();
    assert_eq!(created["api_key"]["scopes"], json!(["conversations"]));

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/api/conversations", session.base))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // A scope the key does not carry is forbidden, even though the role allows it.
    let resp = http
        .get(format!("{}/api/files", session.base))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    let resp = session
        .post("/api/api-keys", &json!({"name": "bad", "scopes": ["nope"]}))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

/// Pull the `token=` value out of the most recent link in a captured mailbox.
fn last_token(path: &std::path::Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let start = text.rfind("token=").expect("mailbox has no link") + "token=".len();
    text[start..]
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn email_verification_and_password_reset() {
    let dir = std::env::temp_dir().join(format!("rustchat-mail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mailbox = dir.join("mail.txt");

    let env = TestEnvBuilder::new()
        .env("MAIL_SEND_COMMAND", format!("cat >> {}", mailbox.display()))
        .env("REQUIRE_EMAIL_VERIFICATION", "true")
        .start()
        .await
        .unwrap();

    let session = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();

    // Registration mails a link, and the account starts out unverified.
    let verify_token = last_token(&mailbox);
    let resp = session.get("/api/conversations").await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    let resp = session
        .post("/api/auth/verify-email", &json!({"token": verify_token}))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    let resp = session.get("/api/conversations").await.unwrap();
    assert!(resp.status().is_success());

    // The token is single-use.
    let resp = session
        .post("/api/auth/verify-email", &json!({"token": verify_token}))
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // A reset link goes out for a known address and never for an unknown one.
    let resp = session
        .post(
            "/api/auth/password-reset",
            &json!({"email": "nobody@example.com"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    let before = std::fs::read_to_string(&mailbox).unwrap().len();

    let resp = session
        .post(
            "/api/auth/password-reset",
            &json!({"email": "a@example.com"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    let first_reset = last_token(&mailbox);
    assert!(std::fs::read_to_string(&mailbox).unwrap().len() > before);

    // Requesting a second link invalidates the first.
    session
        .post(
            "/api/auth/password-reset",
            &json!({"email": "a@example.com"}),
        )
        .await
        .unwrap();
    let second_reset = last_token(&mailbox);
    assert_ne!(first_reset, second_reset);

    let resp = session
        .post(
            "/api/auth/password-reset/confirm",
            &json!({"token": first_reset, "password": "newpassword123"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // A short password is refused before the token is consumed.
    let resp = session
        .post(
            "/api/auth/password-reset/confirm",
            &json!({"token": second_reset, "password": "short"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let resp = session
        .post(
            "/api/auth/password-reset/confirm",
            &json!({"token": second_reset, "password": "newpassword123"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    // The reset token is spent and the password has changed.
    let resp = session
        .post(
            "/api/auth/password-reset/confirm",
            &json!({"token": second_reset, "password": "newpassword123"}),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        Session::login(&env.base_url, "a@example.com", "correct-horse-battery")
            .await
            .is_err()
    );
    Session::login(&env.base_url, "a@example.com", "newpassword123")
        .await
        .unwrap();

    let audit = Session::login(&env.base_url, "a@example.com", "newpassword123")
        .await
        .unwrap()
        .get_expect("/api/admin/audit?limit=50")
        .await
        .unwrap();
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["action"].as_str())
        .collect();
    assert!(
        actions.contains(&"auth.email_verified"),
        "audit: {actions:?}"
    );
    assert!(
        actions.contains(&"auth.password_reset"),
        "audit: {actions:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn account_lockout_and_password_policy() {
    let env = TestEnvBuilder::new()
        .env("LOGIN_MAX_FAILURES", "3")
        .env("LOGIN_LOCKOUT_SECONDS", "1")
        .start()
        .await
        .unwrap();

    // A password from the bundled breached list never makes it into an account.
    let resp = env
        .http
        .post(format!("{}/api/auth/register", env.base_url))
        .json(&json!({"email": "weak@example.com", "password": "password123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let admin = Session::register(&env.base_url, "a@example.com", "correct-horse-battery")
        .await
        .unwrap();
    let user = Session::register(&env.base_url, "b@example.com", "correct-horse-battery")
        .await
        .unwrap();

    async fn wrong(base: &str, email: &str) -> reqwest::StatusCode {
        let resp = reqwest::Client::new()
            .post(format!("{base}/api/auth/login"))
            .json(&json!({"email": email, "password": "wrong-password"}))
            .send()
            .await
            .unwrap();
        resp.status()
    }

    // Two failures are not enough, and a success in between clears the streak.
    for _ in 0..2 {
        assert_eq!(
            wrong(&env.base_url, "b@example.com").await,
            reqwest::StatusCode::UNAUTHORIZED
        );
    }
    Session::login(&env.base_url, "b@example.com", "correct-horse-battery")
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            wrong(&env.base_url, "b@example.com").await,
            reqwest::StatusCode::UNAUTHORIZED
        );
    }

    // The third consecutive failure locks the account out, even for the right
    // password, and the lock is visible to the admin console.
    assert_eq!(
        wrong(&env.base_url, "b@example.com").await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/login", env.base_url))
        .json(&json!({"email": "b@example.com", "password": "correct-horse-battery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    let listed = admin.get_expect("/api/admin/users").await.unwrap();
    let locked = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(user.user_id))
        .expect("user is listed");
    assert!(locked["locked_until"].is_string(), "row: {locked}");

    // The lock expires on its own after LOGIN_LOCKOUT_SECONDS, and the failure
    // streak starts over rather than re-locking on the next typo.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(
        wrong(&env.base_url, "b@example.com").await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    Session::login(&env.base_url, "b@example.com", "correct-horse-battery")
        .await
        .unwrap();

    // Lock again, then clear it early from the admin console.
    for _ in 0..3 {
        assert_eq!(
            wrong(&env.base_url, "b@example.com").await,
            reqwest::StatusCode::UNAUTHORIZED
        );
    }
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/login", env.base_url))
        .json(&json!({"email": "b@example.com", "password": "correct-horse-battery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    admin
        .patch_expect(
            &format!("/api/admin/users/{}", user.user_id),
            &json!({"unlock": true}),
        )
        .await
        .unwrap();
    Session::login(&env.base_url, "b@example.com", "correct-horse-battery")
        .await
        .unwrap();

    // Both the lock-out and the manual unlock are audited.
    let audit = admin
        .get_expect("/api/admin/audit?limit=100")
        .await
        .unwrap();
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["action"].as_str())
        .collect();
    assert!(
        actions.contains(&"auth.account_locked"),
        "audit: {actions:?}"
    );
    assert!(actions.contains(&"admin.user.unlock"), "audit: {actions:?}");
}
