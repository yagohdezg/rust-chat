use chat_core::Result;
use chat_store::{EmbeddingChunk, RetrievedChunk, Scope, VectorStore};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Brute-force vector search over an SQLite `chunks` table.
///
/// Embeddings are stored as little-endian f32 blobs and scored with a cosine
/// scan in Rust. That is O(n) per query, which is fine for the personal-scale
/// corpora SQLite targets; a `sqlite-vec` extension could replace this later
/// without touching the `VectorStore` seam.
pub struct SqliteVectorStore {
    pool: SqlitePool,
}

impl SqliteVectorStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn embedding_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn embedding_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

#[async_trait::async_trait]
impl VectorStore for SqliteVectorStore {
    async fn delete_for_file(&self, file_id: Uuid) -> Result<()> {
        sqlx::query("delete from chunks where file_id = ?")
            .bind(file_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_chunks(&self, chunks: &[EmbeddingChunk]) -> Result<()> {
        for chunk in chunks {
            sqlx::query(
                "insert into chunks (id, user_id, conversation_id, file_id, content, embedding, created_at)
                 values (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4())
            .bind(chunk.user_id)
            .bind(chunk.conversation_id)
            .bind(chunk.file_id)
            .bind(&chunk.content)
            .bind(embedding_to_bytes(&chunk.embedding))
            .bind(Utc::now())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn search(&self, scope: &Scope, query: &[f32], k: usize) -> Result<Vec<RetrievedChunk>> {
        let rows: Vec<(String, Option<Uuid>, Vec<u8>)> = match scope.conversation_id {
            Some(conversation_id) => {
                sqlx::query_as(
                    "select content, file_id, embedding from chunks
                     where user_id = ? and conversation_id = ?",
                )
                .bind(scope.user_id)
                .bind(conversation_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as("select content, file_id, embedding from chunks where user_id = ?")
                    .bind(scope.user_id)
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        let mut scored: Vec<RetrievedChunk> = rows
            .into_iter()
            .map(|(content, file_id, bytes)| {
                let embedding = embedding_from_bytes(&bytes);
                RetrievedChunk {
                    content,
                    file_id,
                    score: cosine_similarity(query, &embedding),
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(k);
        Ok(scored)
    }
}
