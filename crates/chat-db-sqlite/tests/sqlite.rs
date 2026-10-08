//! Smoke test proving the SQLite backend behind the `chat-store` traits:
//! relational CRUD through `Store` and cosine search through `VectorStore`.

use chat_db_sqlite::{SqliteStore, SqliteVectorStore};
use chat_store::{EmbeddingChunk, ProviderModel, Scope, Store, VectorStore};
use uuid::Uuid;

async fn temp_store() -> (SqliteStore, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("rustchat-test-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let secrets = std::sync::Arc::new(
        chat_core::SecretCipher::derive_from_secret("test-secret").expect("cipher"),
    );
    let store = SqliteStore::connect(&url, secrets)
        .await
        .expect("connect sqlite");
    store.migrate().await.expect("run migrations");
    (store, path)
}

#[tokio::test]
async fn relational_crud_roundtrip() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("ada@example.com", Some("Ada"), "hash", "user")
        .await
        .expect("create user");
    assert_eq!(
        store
            .find_user_by_email("ada@example.com")
            .await
            .unwrap()
            .unwrap()
            .id,
        user.id
    );

    let conversation = store
        .create_conversation(user.id, None, "hello")
        .await
        .expect("create conversation");
    store
        .insert_message(conversation.id, "user", Some("hi there"), None, None)
        .await
        .expect("insert message");
    store
        .insert_message(conversation.id, "assistant", Some("hello!"), None, None)
        .await
        .expect("insert message");

    let messages = store.list_messages(conversation.id).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content.as_deref(), Some("hi there"));

    // Ownership scoping: another user cannot read the conversation.
    let other = store
        .create_user("grace@example.com", None, "hash", "user")
        .await
        .unwrap();
    assert!(store
        .get_conversation(conversation.id, other.id)
        .await
        .is_err());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn agent_and_provider_loading() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("agent@example.com", None, "hash", "user")
        .await
        .unwrap();
    let other = store
        .create_user("other@example.com", None, "hash", "user")
        .await
        .unwrap();

    // A global provider (`user_id` null) is visible to everyone; a private one
    // only to its owner. A user's own providers list before globals.
    let global = store
        .create_provider(None, "global", "openai", "https://api.example.com", None)
        .await
        .unwrap();
    let private = store
        .create_provider(
            Some(user.id),
            "private",
            "openai",
            "https://api.example.com",
            Some("sk-test"),
        )
        .await
        .unwrap();
    assert_eq!(private.api_key.as_deref(), Some("sk-test"));
    assert!(store
        .get_provider(global.id, other.id)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_provider(private.id, other.id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_provider(private.id, user.id)
        .await
        .unwrap()
        .is_some());
    let listed = store.list_providers(user.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, private.id, "own providers come first");
    assert_eq!(store.list_providers(other.id).await.unwrap().len(), 1);
    assert_eq!(store.count_users().await.unwrap(), 2);

    // Deleting with the wrong owner leaves the row in place.
    store
        .delete_provider(private.id, Some(other.id))
        .await
        .unwrap();
    assert!(store
        .get_provider(private.id, user.id)
        .await
        .unwrap()
        .is_some());

    // An agent round-trips, including the JSON tool list and the bool flag, and
    // stays scoped to its owner.
    let created = store
        .create_agent(
            user.id,
            chat_store::AgentDraft {
                name: "helper".into(),
                instructions: Some("be concise".into()),
                provider_id: Some(private.id),
                model: Some("gpt-4o-mini".into()),
                tools: vec!["execute_code".to_string()],
                sandbox_enabled: true,
            },
        )
        .await
        .expect("create agent");
    assert_eq!(created.tools.0, vec!["execute_code".to_string()]);
    assert!(created.sandbox_enabled);

    let agents = store.list_agents(user.id).await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, created.id);
    assert!(store.list_agents(other.id).await.unwrap().is_empty());

    let agent = store
        .get_agent(created.id, user.id)
        .await
        .unwrap()
        .expect("agent row");
    assert_eq!(agent.name, "helper");
    assert_eq!(agent.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(agent.instructions.as_deref(), Some("be concise"));
    assert_eq!(agent.tools.0, vec!["execute_code".to_string()]);
    assert!(agent.sandbox_enabled);
    assert!(store
        .get_agent(created.id, other.id)
        .await
        .unwrap()
        .is_none());

    // Deletion is owner-scoped: a foreign user cannot remove it.
    store.delete_agent(created.id, other.id).await.unwrap();
    assert!(store
        .get_agent(created.id, user.id)
        .await
        .unwrap()
        .is_some());
    store.delete_agent(created.id, user.id).await.unwrap();
    assert!(store
        .get_agent(created.id, user.id)
        .await
        .unwrap()
        .is_none());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn model_catalog_is_cached_per_provider() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("models@example.com", None, "hash", "user")
        .await
        .unwrap();
    let provider = store
        .create_provider(
            Some(user.id),
            "openai",
            "openai",
            "https://api.example.com",
            None,
        )
        .await
        .unwrap();

    assert!(store
        .list_provider_models(provider.id)
        .await
        .unwrap()
        .is_empty());

    let now = chrono::Utc::now();
    let models = vec![
        ProviderModel {
            provider_id: provider.id,
            id: "gpt-4o".into(),
            owned_by: Some("openai".into()),
            fetched_at: now,
        },
        ProviderModel {
            provider_id: provider.id,
            id: "gpt-4o-mini".into(),
            owned_by: None,
            fetched_at: now,
        },
    ];
    store
        .replace_provider_models(provider.id, &models)
        .await
        .unwrap();

    let listed = store.list_provider_models(provider.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "gpt-4o");

    // Replace is authoritative, not additive.
    store
        .replace_provider_models(provider.id, &models[..1])
        .await
        .unwrap();
    assert_eq!(
        store.list_provider_models(provider.id).await.unwrap().len(),
        1
    );

    // Deleting the provider cascades to its catalog.
    store
        .delete_provider(provider.id, Some(user.id))
        .await
        .unwrap();
    assert!(store
        .list_provider_models(provider.id)
        .await
        .unwrap()
        .is_empty());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn admin_user_summaries_and_provider_update() {
    let (store, path) = temp_store().await;

    let admin = store
        .create_user("admin@example.com", Some("Admin"), "hash", "admin")
        .await
        .unwrap();
    let user = store
        .create_user("user@example.com", None, "hash", "user")
        .await
        .unwrap();

    store
        .create_conversation(user.id, None, "hi")
        .await
        .unwrap();
    store
        .create_provider(
            Some(user.id),
            "p",
            "openai",
            "https://api.example.com",
            Some("sk-old"),
        )
        .await
        .unwrap();

    let summaries = store.list_user_summaries().await.unwrap();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id, admin.id, "oldest account first");
    let target = summaries.iter().find(|s| s.id == user.id).unwrap();
    assert_eq!(target.role, "user");
    assert!(!target.disabled);
    assert!(target.last_seen_at.is_none());
    assert_eq!(target.conversation_count, 1);
    assert_eq!(target.provider_count, 1);
    assert_eq!(target.agent_count, 0);

    // Role and enable/disable are reflected in the next summary.
    store.set_user_role(user.id, "admin").await.unwrap();
    store.set_user_disabled(user.id, true).await.unwrap();
    store.touch_user_last_seen(user.id).await.unwrap();
    let summaries = store.list_user_summaries().await.unwrap();
    let target = summaries.iter().find(|s| s.id == user.id).unwrap();
    assert_eq!(target.role, "admin");
    assert!(target.disabled);
    assert!(target.last_seen_at.is_some());

    // Provider updates rotate the key (still encrypted at rest) and can clear it.
    let mut providers = store.list_providers(user.id).await.unwrap();
    let provider = providers.remove(0);
    let updated = store
        .update_provider(
            provider.id,
            "p2",
            "custom",
            "https://new.example.com",
            Some("sk-new"),
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "p2");
    assert_eq!(updated.kind, "custom");
    assert_eq!(updated.base_url, "https://new.example.com");
    assert_eq!(updated.api_key.as_deref(), Some("sk-new"));

    let cleared = store
        .update_provider(provider.id, "p2", "custom", "https://new.example.com", None)
        .await
        .unwrap();
    assert!(cleared.api_key.is_none());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn provider_credentials_round_trip() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("keyed@example.com", None, "hash", "user")
        .await
        .unwrap();
    // An admin-provisioned global provider with no shared key.
    let provider = store
        .create_provider(None, "shared", "openai", "https://api.example.com", None)
        .await
        .unwrap();

    assert!(store
        .get_provider_credential(user.id, provider.id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .list_provider_credential_ids(user.id)
        .await
        .unwrap()
        .is_empty());

    store
        .set_provider_credential(user.id, provider.id, Some("sk-user"))
        .await
        .unwrap();
    assert_eq!(
        store
            .get_provider_credential(user.id, provider.id)
            .await
            .unwrap()
            .as_deref(),
        Some("sk-user")
    );
    assert_eq!(
        store.list_provider_credential_ids(user.id).await.unwrap(),
        vec![provider.id]
    );

    // Upsert replaces the stored key.
    store
        .set_provider_credential(user.id, provider.id, Some("sk-new"))
        .await
        .unwrap();
    assert_eq!(
        store
            .get_provider_credential(user.id, provider.id)
            .await
            .unwrap()
            .as_deref(),
        Some("sk-new")
    );

    // `None` clears it.
    store
        .set_provider_credential(user.id, provider.id, None)
        .await
        .unwrap();
    assert!(store
        .get_provider_credential(user.id, provider.id)
        .await
        .unwrap()
        .is_none());

    // Deleting the provider cascades to its credentials.
    store
        .set_provider_credential(user.id, provider.id, Some("sk-x"))
        .await
        .unwrap();
    store.delete_provider(provider.id, None).await.unwrap();
    assert!(store
        .list_provider_credential_ids(user.id)
        .await
        .unwrap()
        .is_empty());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn delete_user_cascades_to_resources() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("gone@example.com", None, "hash", "user")
        .await
        .unwrap();
    let keeper = store
        .create_user("keep@example.com", None, "hash", "user")
        .await
        .unwrap();
    store.create_conversation(user.id, None, "x").await.unwrap();
    store
        .create_provider(Some(user.id), "p", "openai", "https://x", None)
        .await
        .unwrap();
    assert_eq!(store.count_users().await.unwrap(), 2);

    store.delete_user(user.id).await.unwrap();

    assert_eq!(store.count_users().await.unwrap(), 1);
    assert!(store.get_user(user.id).await.unwrap().is_none());
    assert!(store.list_conversations(user.id).await.unwrap().is_empty());
    assert!(store.list_providers(user.id).await.unwrap().is_empty());
    assert!(store.get_user(keeper.id).await.unwrap().is_some());

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn refresh_tokens_rotate_and_audit_is_recorded() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("rotate@example.com", None, "hash", "user")
        .await
        .unwrap();

    let expires = chrono::Utc::now() + chrono::Duration::days(30);
    store
        .create_refresh_token(user.id, "hash-a", expires)
        .await
        .unwrap();

    let row = store
        .get_refresh_token("hash-a")
        .await
        .unwrap()
        .expect("refresh row");
    assert_eq!(row.user_id, user.id);
    assert!(row.revoked_at.is_none());

    // Revoking marks it revoked, idempotently.
    store.revoke_refresh_token("hash-a").await.unwrap();
    store.revoke_refresh_token("hash-a").await.unwrap();
    assert!(store
        .get_refresh_token("hash-a")
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());

    // A user-wide revoke sweeps every outstanding token.
    store
        .create_refresh_token(user.id, "hash-b", expires)
        .await
        .unwrap();
    store.revoke_user_refresh_tokens(user.id).await.unwrap();
    assert!(store
        .get_refresh_token("hash-b")
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(store.get_refresh_token("missing").await.unwrap().is_none());

    // Audit rows round-trip, newest first, with typed metadata.
    store
        .record_audit(&chat_store::AuditEntry {
            actor_id: Some(user.id),
            action: "auth.login".into(),
            target_type: Some("user".into()),
            target_id: Some(user.id.to_string()),
            metadata: Some(serde_json::json!({ "email": "rotate@example.com" })),
            ip: Some("127.0.0.1".into()),
        })
        .await
        .unwrap();
    store
        .record_audit(&chat_store::AuditEntry {
            actor_id: Some(user.id),
            action: "sandbox.exec".into(),
            target_type: Some("sandbox".into()),
            target_id: None,
            metadata: None,
            ip: None,
        })
        .await
        .unwrap();

    let logs = store.list_audit_logs(10).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].action, "sandbox.exec", "newest first");
    let login = logs.iter().find(|l| l.action == "auth.login").unwrap();
    assert_eq!(login.actor_id, Some(user.id));
    assert_eq!(login.ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(
        login.metadata.as_ref().unwrap().0["email"],
        "rotate@example.com"
    );

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn vector_search_returns_nearest_chunk() {
    let (store, path) = temp_store().await;

    let user = store
        .create_user("vector@example.com", None, "hash", "user")
        .await
        .unwrap();
    let conversation = store
        .create_conversation(user.id, None, "docs")
        .await
        .unwrap();

    let vectors = SqliteVectorStore::new(store.pool());
    let file_id = Uuid::new_v4();
    vectors
        .insert_chunks(&[
            EmbeddingChunk {
                user_id: user.id,
                conversation_id: Some(conversation.id),
                file_id: Some(file_id),
                content: "cats are independent".into(),
                embedding: vec![1.0, 0.0, 0.0],
            },
            EmbeddingChunk {
                user_id: user.id,
                conversation_id: Some(conversation.id),
                file_id: Some(file_id),
                content: "dogs are loyal".into(),
                embedding: vec![0.0, 1.0, 0.0],
            },
        ])
        .await
        .unwrap();

    let scope = Scope {
        user_id: user.id,
        conversation_id: Some(conversation.id),
    };
    let results = vectors.search(&scope, &[1.0, 0.0, 0.0], 1).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "cats are independent");
    assert!(results[0].score > 0.99);

    // Re-indexing the same file replaces its chunks.
    vectors.delete_for_file(file_id).await.unwrap();
    assert!(vectors
        .search(&scope, &[1.0, 0.0, 0.0], 5)
        .await
        .unwrap()
        .is_empty());

    drop(store);
    let _ = std::fs::remove_file(path);
}
