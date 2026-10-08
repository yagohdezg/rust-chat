//! Smoke test proving the SQLite backend behind the `chat-store` traits:
//! relational CRUD through `Store` and cosine search through `VectorStore`.

use chat_db_sqlite::{SqliteStore, SqliteVectorStore};
use chat_store::{EmbeddingChunk, Scope, Store, VectorStore};
use uuid::Uuid;

async fn temp_store() -> (SqliteStore, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("rustchat-test-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = SqliteStore::connect(&url).await.expect("connect sqlite");
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
