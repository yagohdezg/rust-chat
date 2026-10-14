mod common;

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
    let first = Session::register(&env.base_url, "admin@example.com", "password123")
        .await
        .unwrap();
    assert_eq!(first.role, "admin");

    let second = Session::register(&env.base_url, "user@example.com", "password123")
        .await
        .unwrap();
    assert_eq!(second.role, "user");
}

#[tokio::test]
async fn login_round_trip() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let registered = Session::register(&env.base_url, "a@example.com", "password123")
        .await
        .unwrap();
    let logged_in = Session::login(&env.base_url, "a@example.com", "password123")
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
    let session = Session::register(&env.base_url, "a@example.com", "password123")
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
    let session = Session::register(&env.base_url, "a@example.com", "password123")
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
async fn provider_lifecycle() {
    let env = TestEnvBuilder::new().start().await.unwrap();
    let session = Session::register(&env.base_url, "a@example.com", "password123")
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
    let alice = Session::register(&env.base_url, "alice@example.com", "password123")
        .await
        .unwrap();
    let bob = Session::register(&env.base_url, "bob@example.com", "password123")
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

    let admin = Session::register(&env.base_url, "admin@example.com", "password123")
        .await
        .unwrap();
    assert_eq!(admin.role, "admin");
    let user = Session::register(&env.base_url, "user@example.com", "password123")
        .await
        .unwrap();
    assert_eq!(user.role, "user");

    let logged_in = Session::login(&env.base_url, "admin@example.com", "password123")
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
