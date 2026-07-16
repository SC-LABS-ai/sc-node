//! Deterministic, offline `EmbeddingProvider` for tests and local-only
//! workflows. Never touches the network.

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::error::MemoryError;
use crate::traits::EmbeddingProvider;

/// Hashes text into a fixed-dimension vector using SHA-256.
///
/// Deterministic: the same text always produces the same vector, and
/// different texts (very likely) produce different vectors, which is
/// enough to exercise storage, persistence, and similarity ranking in
/// tests without any real embedding model or network access.
pub struct FakeEmbeddingProvider {
    dim: usize,
    model_id: String,
    version: u32,
}

impl FakeEmbeddingProvider {
    /// Create a provider with the default model id/version.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            model_id: "fake-hash-v1".to_string(),
            version: 1,
        }
    }

    /// Create a provider that reports a custom model id/version, useful
    /// for testing reindex-detection when the "model" changes.
    pub fn with_model(dim: usize, model_id: impl Into<String>, version: u32) -> Self {
        Self {
            dim,
            model_id: model_id.into(),
            version,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FakeEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn version(&self) -> u32 {
        self.version
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        Ok(hash_embed(text, self.dim))
    }
}

/// Deterministically expand `text` into `dim` floats in `[-1.0, 1.0]`.
///
/// Repeatedly hashes `text || counter` with SHA-256 and reinterprets the
/// digest bytes as `u32`s, mapped into `[-1, 1]`, until `dim` values have
/// been produced. Pure function of its inputs: no RNG, no I/O.
pub fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(dim);
    let mut counter: u32 = 0;
    while out.len() < dim {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        hasher.update(counter.to_le_bytes());
        let digest = hasher.finalize();
        for chunk in digest.chunks(4) {
            if out.len() >= dim {
                break;
            }
            let mut buf = [0u8; 4];
            buf[..chunk.len()].copy_from_slice(chunk);
            let v = u32::from_le_bytes(buf);
            let f = (v as f64 / u32::MAX as f64) * 2.0 - 1.0;
            out.push(f as f32);
        }
        counter += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embed_is_deterministic() {
        let provider = FakeEmbeddingProvider::new(16);
        let a = provider.embed("hello world").await.unwrap();
        let b = provider.embed("hello world").await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_embed_has_requested_dimension() {
        let provider = FakeEmbeddingProvider::new(37);
        let v = provider.embed("anything").await.unwrap();
        assert_eq!(v.len(), 37);
    }

    #[tokio::test]
    async fn test_embed_differs_for_different_text() {
        let provider = FakeEmbeddingProvider::new(16);
        let a = provider.embed("hello").await.unwrap();
        let b = provider.embed("goodbye").await.unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_custom_model_metadata() {
        let provider = FakeEmbeddingProvider::with_model(4, "custom-model", 7);
        assert_eq!(provider.model_id(), "custom-model");
        assert_eq!(provider.version(), 7);
        assert_eq!(provider.dimension(), 4);
    }
}
