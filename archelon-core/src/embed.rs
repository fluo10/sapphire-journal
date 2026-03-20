//! Text embedding providers.
//!
//! Converts text (entry title + body) into float vectors used for semantic
//! similarity search via the sqlite-vec backend.
//!
//! The supported providers are:
//!
//! - **`"openai"`** — OpenAI-compatible `/v1/embeddings` endpoint.
//! - **`"ollama"`** — Ollama `/api/embed` endpoint.
//! - **`"fastembed"`** — Local ONNX inference via the `fastembed` crate.
//!   No server required; model weights are downloaded from Hugging Face
//!   on first use and cached under `~/.cache/huggingface/hub/`.

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
        #[cfg(feature = "fastembed-embed")]
        "fastembed" => embed_fastembed(config, texts),
        other => Err(Error::Embed(format!(
            "unknown embedding provider `{other}`; supported values: openai, ollama{}",
            if cfg!(feature = "fastembed-embed") { ", fastembed" } else { "" }
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

// ── fastembed (local ONNX) ────────────────────────────────────────────────────

/// Supported fastembed model names and their output dimensions.
///
/// | Config `model` value   | Dimension |
/// |------------------------|-----------|
/// | `AllMiniLML6V2`        | 384       |
/// | `BGESmallENV15`        | 384       |
/// | `BGEBaseENV15`         | 768       |
/// | `BGELargeENV15`        | 1024      |
/// | `NomicEmbedTextV1`     | 768       |
/// | `NomicEmbedTextV15`    | 768       |
/// | `MultilingualE5Small`  | 384       |
/// | `MultilingualE5Base`   | 768       |
/// | `MultilingualE5Large`  | 1024      |
#[cfg(feature = "fastembed-embed")]
fn embed_fastembed(config: &EmbeddingConfig, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

    let model_variant = match config.model.as_str() {
        "AllMiniLML6V2"       => EmbeddingModel::AllMiniLML6V2,
        "BGESmallENV15"       => EmbeddingModel::BGESmallENV15,
        "BGEBaseENV15"        => EmbeddingModel::BGEBaseENV15,
        "BGELargeENV15"       => EmbeddingModel::BGELargeENV15,
        "NomicEmbedTextV1"    => EmbeddingModel::NomicEmbedTextV1,
        "NomicEmbedTextV15"   => EmbeddingModel::NomicEmbedTextV15,
        "MultilingualE5Small" => EmbeddingModel::MultilingualE5Small,
        "MultilingualE5Base"  => EmbeddingModel::MultilingualE5Base,
        "MultilingualE5Large" => EmbeddingModel::MultilingualE5Large,
        other => {
            return Err(Error::Embed(format!(
                "unknown fastembed model `{other}`; \
                 supported: AllMiniLML6V2, BGESmallENV15, BGEBaseENV15, BGELargeENV15, \
                 NomicEmbedTextV1, NomicEmbedTextV15, \
                 MultilingualE5Small, MultilingualE5Base, MultilingualE5Large"
            )))
        }
    };

    let cache_dir = xdg_cache_home().join("archelon").join("fastembed");

    let model = TextEmbedding::try_new(
        InitOptions::new(model_variant)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(true),
    )
    .map_err(|e| Error::Embed(format!("failed to load fastembed model: {e}")))?;

    let texts_owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    model
        .embed(texts_owned, None)
        .map_err(|e| Error::Embed(format!("fastembed embedding failed: {e}")))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn xdg_cache_home() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".cache");
    }
    std::env::temp_dir()
}

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
