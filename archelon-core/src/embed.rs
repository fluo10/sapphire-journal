//! Text embedding providers.
//!
//! Converts text (entry title + body) into float vectors used for semantic
//! similarity search via the sqlite-vec backend.
//!
//! All network calls are made synchronously via [`ureq`].  The two supported
//! providers are:
//!
//! - **`"openai"`** — OpenAI-compatible `/v1/embeddings` endpoint.
//! - **`"ollama"`** — Ollama `/api/embed` endpoint.

use crate::{
    error::{Error, Result},
    user_config::EmbeddingConfig,
};

/// Generate embeddings for a batch of texts.
///
/// Returns one `Vec<f32>` per input text, in the same order.
/// Returns an empty `Vec` when `texts` is empty (no API call is made).
pub fn embed_texts(config: &EmbeddingConfig, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    match config.provider.as_str() {
        "openai" => embed_openai(config, texts),
        "ollama" => embed_ollama(config, texts),
        other => Err(Error::Embed(format!(
            "unknown embedding provider `{other}`; supported values: openai, ollama"
        ))),
    }
}

// ── OpenAI-compatible ─────────────────────────────────────────────────────────

fn embed_openai(config: &EmbeddingConfig, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let api_key_env = config.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
    let api_key = std::env::var(api_key_env).map_err(|_| {
        Error::Embed(format!("environment variable `{api_key_env}` is not set"))
    })?;

    let base_url = config.base_url.as_deref().unwrap_or("https://api.openai.com");
    let url = format!("{base_url}/v1/embeddings");

    let body = serde_json::json!({
        "model": config.model,
        "input": texts,
    });

    let response: serde_json::Value = ureq::post(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| Error::Embed(e.to_string()))?
        .into_json()
        .map_err(|e| Error::Embed(e.to_string()))?;

    parse_openai_response(&response, texts.len())
}

fn parse_openai_response(
    response: &serde_json::Value,
    expected: usize,
) -> Result<Vec<Vec<f32>>> {
    let data = response["data"]
        .as_array()
        .ok_or_else(|| Error::Embed("unexpected OpenAI response: missing `data` array".into()))?;

    let mut results = vec![Vec::new(); expected];
    for item in data {
        let index = item["index"]
            .as_u64()
            .ok_or_else(|| Error::Embed("missing `index` in embedding object".into()))?
            as usize;
        let vec = parse_float_array(&item["embedding"])?;
        if index < results.len() {
            results[index] = vec;
        }
    }
    Ok(results)
}

// ── Ollama ────────────────────────────────────────────────────────────────────

fn embed_ollama(config: &EmbeddingConfig, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let base_url = config.base_url.as_deref().unwrap_or("http://localhost:11434");
    let url = format!("{base_url}/api/embed");

    let body = serde_json::json!({
        "model": config.model,
        "input": texts,
    });

    let response: serde_json::Value = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| Error::Embed(e.to_string()))?
        .into_json()
        .map_err(|e| Error::Embed(e.to_string()))?;

    response["embeddings"]
        .as_array()
        .ok_or_else(|| Error::Embed("unexpected Ollama response: missing `embeddings` array".into()))?
        .iter()
        .map(|arr| parse_float_array(arr))
        .collect()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_float_array(value: &serde_json::Value) -> Result<Vec<f32>> {
    value
        .as_array()
        .ok_or_else(|| Error::Embed("embedding value is not a JSON array".into()))?
        .iter()
        .map(|v| {
            v.as_f64()
                .map(|f| f as f32)
                .ok_or_else(|| Error::Embed("non-numeric value in embedding vector".into()))
        })
        .collect()
}
