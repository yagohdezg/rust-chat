use chat_core::Result;
use chat_store::{EmbeddingChunk, RetrievedChunk, Scope, VectorStore};
use sqlx::PgPool;
use uuid::Uuid;

/// `pgvector`-backed similarity search.
///
/// Vectors are bound as their text representation (`[1,2,3]`) and cast to
/// `vector` in SQL, which avoids depending on a driver-specific vector type.
pub struct PgVectorStore {
    pool: PgPool,
}

impl PgVectorStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Render a vector as the text literal `[1,2,3]` accepted by `pgvector`.
fn vector_literal(values: &[f32]) -> String {
    let mut out = String::with_capacity(values.len() * 8 + 2);
    out.push('[');
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&v.to_string());
    }
    out.push(']');
    out
}

#[async_trait::async_trait]
impl VectorStore for PgVectorStore {
    async fn delete_for_file(&self, file_id: Uuid) -> Result<()> {
        sqlx::query("delete from embeddings where file_id = $1")
            .bind(file_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_chunks(&self, chunks: &[EmbeddingChunk]) -> Result<()> {
        for chunk in chunks {
            sqlx::query(
                "insert into embeddings (user_id, conversation_id, file_id, content, embedding)
                 values ($1, $2, $3, $4, $5::vector)",
            )
            .bind(chunk.user_id)
            .bind(chunk.conversation_id)
            .bind(chunk.file_id)
            .bind(&chunk.content)
            .bind(vector_literal(&chunk.embedding))
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn search(&self, scope: &Scope, query: &[f32], k: usize) -> Result<Vec<RetrievedChunk>> {
        let literal = vector_literal(query);
        let k = k as i64;

        let rows: Vec<(String, Option<Uuid>, f64)> = match scope.conversation_id {
            Some(conversation_id) => {
                sqlx::query_as(
                    "select content, file_id, (1 - (embedding <=> $1::vector))::float8 as score
                     from embeddings
                     where user_id = $2 and conversation_id = $3
                     order by embedding <=> $1::vector
                     limit $4",
                )
                .bind(&literal)
                .bind(scope.user_id)
                .bind(conversation_id)
                .bind(k)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "select content, file_id, (1 - (embedding <=> $1::vector))::float8 as score
                     from embeddings
                     where user_id = $2
                     order by embedding <=> $1::vector
                     limit $3",
                )
                .bind(&literal)
                .bind(scope.user_id)
                .bind(k)
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(|(content, file_id, score)| RetrievedChunk {
                content,
                file_id,
                score: score as f32,
            })
            .collect())
    }
}
