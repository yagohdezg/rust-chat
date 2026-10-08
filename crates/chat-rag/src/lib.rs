//! Retrieval-Augmented Generation: chunk -> embed -> store, and
//! query -> embed -> search -> format context.
//!
//! Depends only on the `chat-store` traits, so it works with any backend.

pub mod chunker;
pub mod embedder;

use std::sync::Arc;

use chat_core::{ChatError, Result};
use chat_store::{Embedder, EmbeddingChunk, RetrievedChunk, Scope, VectorStore};
use uuid::Uuid;

pub use chunker::chunk_text;
pub use embedder::OpenAiEmbedder;

/// Texts per embedding request.
const EMBED_BATCH: usize = 64;

/// End-to-end RAG pipeline over a [`VectorStore`] and an [`Embedder`].
#[derive(Clone)]
pub struct RagPipeline {
    vectors: Arc<dyn VectorStore>,
    embedder: Arc<dyn Embedder>,
    pub top_k: usize,
    pub chunk_chars: usize,
    pub chunk_overlap: usize,
}

impl RagPipeline {
    pub fn new(
        vectors: Arc<dyn VectorStore>,
        embedder: Arc<dyn Embedder>,
        top_k: usize,
        chunk_chars: usize,
        chunk_overlap: usize,
    ) -> Self {
        Self {
            vectors,
            embedder,
            top_k,
            chunk_chars,
            chunk_overlap,
        }
    }

    /// Extract, chunk, embed and persist the contents of an uploaded file.
    /// Returns the number of chunks indexed.
    pub async fn index_file(
        &self,
        user_id: Uuid,
        conversation_id: Option<Uuid>,
        file_id: Uuid,
        bytes: &[u8],
    ) -> Result<usize> {
        let text = extract_text(bytes)?;
        self.index_text(user_id, conversation_id, Some(file_id), &text)
            .await
    }

    /// Index already-extracted text.
    pub async fn index_text(
        &self,
        user_id: Uuid,
        conversation_id: Option<Uuid>,
        file_id: Option<Uuid>,
        text: &str,
    ) -> Result<usize> {
        let chunks = chunk_text(text, self.chunk_chars, self.chunk_overlap);
        if chunks.is_empty() {
            return Ok(0);
        }

        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(EMBED_BATCH) {
            let vectors = self.embedder.embed(batch).await?;
            if vectors.len() != batch.len() {
                return Err(ChatError::Internal(anyhow::anyhow!(
                    "embedder returned {} vectors for {} inputs",
                    vectors.len(),
                    batch.len()
                )));
            }
            embeddings.extend(vectors);
        }

        let records: Vec<EmbeddingChunk> = chunks
            .into_iter()
            .zip(embeddings)
            .map(|(content, embedding)| EmbeddingChunk {
                user_id,
                conversation_id,
                file_id,
                content,
                embedding,
            })
            .collect();

        // Re-indexing a file replaces its previous chunks.
        if let Some(file_id) = file_id {
            self.vectors.delete_for_file(file_id).await?;
        }
        self.vectors.insert_chunks(&records).await?;
        Ok(records.len())
    }

    /// Retrieve the most relevant chunks for a query.
    pub async fn retrieve(&self, scope: &Scope, query: &str) -> Result<Vec<RetrievedChunk>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut vectors = self.embedder.embed(&[query.to_string()]).await?;
        let Some(query_vector) = vectors.pop() else {
            return Ok(Vec::new());
        };
        self.vectors.search(scope, &query_vector, self.top_k).await
    }

    /// Render retrieved chunks as a system-message preamble.
    pub fn format_context(chunks: &[RetrievedChunk]) -> String {
        if chunks.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "The user has attached documents. Use the most relevant excerpts below \
             to answer, and say when the answer is not present.\n",
        );
        for (i, chunk) in chunks.iter().enumerate() {
            out.push_str(&format!("\n[{}] {}\n", i + 1, chunk.content));
        }
        out
    }
}

/// Extract UTF-8 text from an uploaded file.
///
/// Binary formats (PDF, DOCX, ...) are rejected for now; those need dedicated
/// parsers and would be added as a separate extraction step.
pub fn extract_text(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        ChatError::BadRequest(
            "file is not valid UTF-8; only text-like files can be indexed yet".into(),
        )
    })
}
