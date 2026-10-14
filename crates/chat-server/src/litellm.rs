//! Importer for LiteLLM provider configuration.
//!
//! A LiteLLM proxy is described either by a `config.yaml` whose `model_list`
//! maps friendly model names onto upstreams, or by the `/model/info` endpoint
//! of a running proxy. Both carry the same per-model shape, so the importer
//! parses either into instance-wide (global) providers: entries are grouped by
//! upstream kind and `api_base`, and each group becomes one provider carrying
//! its model names.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use chat_core::ChatError;

/// A provider derived from a LiteLLM config, ready to be upserted.
pub struct ImportedProvider {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub models: Vec<String>,
    /// The upstream key shared by every entry of the group, when there is one.
    pub api_key: Option<String>,
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    model_list: Vec<Entry>,
}

#[derive(Deserialize, Default)]
struct Entry {
    #[serde(default)]
    model_name: String,
    #[serde(default)]
    litellm_params: Params,
}

#[derive(Deserialize, Default)]
struct Params {
    #[serde(default)]
    model: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_base: Option<String>,
}

#[derive(Deserialize)]
struct ModelInfo {
    #[serde(default)]
    data: Vec<Entry>,
}

/// Parse the `model_list` of a LiteLLM `config.yaml`.
pub fn parse_model_list(yaml: &str) -> Result<Vec<ImportedProvider>, ChatError> {
    let config: ModelList = serde_yaml::from_str(yaml)
        .map_err(|err| ChatError::BadRequest(format!("invalid LiteLLM config: {err}")))?;
    group(config.model_list)
}

/// Fetch a running proxy's `/model/info` and derive its providers.
pub async fn fetch_model_info(
    proxy_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ImportedProvider>, ChatError> {
    let url = format!("{}/model/info", proxy_url.trim_end_matches('/'));
    let mut request = reqwest::Client::new().get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|err| ChatError::Upstream(format!("model/info request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(ChatError::Upstream(format!(
            "model/info returned {}",
            response.status()
        )));
    }
    let info: ModelInfo = response
        .json()
        .await
        .map_err(|err| ChatError::Upstream(format!("model/info returned invalid JSON: {err}")))?;
    group(info.data)
}

/// Collapse entries that share an upstream kind and `api_base` into one
/// provider each, preserving model order and dropping duplicates.
fn group(entries: Vec<Entry>) -> Result<Vec<ImportedProvider>, ChatError> {
    if entries.is_empty() {
        return Err(ChatError::BadRequest(
            "the LiteLLM config lists no models".into(),
        ));
    }
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    for entry in entries {
        let model_name = entry.model_name.trim();
        if model_name.is_empty() {
            return Err(ChatError::BadRequest(
                "a LiteLLM entry has no model_name".into(),
            ));
        }
        let base_url = entry
            .litellm_params
            .api_base
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .ok_or_else(|| {
                ChatError::BadRequest(format!(
                    "LiteLLM entry `{model_name}` has no litellm_params.api_base"
                ))
            })?;
        let kind = kind_for(&entry.litellm_params.model);
        let group = groups
            .entry((kind.to_string(), base_url.to_string()))
            .or_default();
        if !group.models.iter().any(|model| model == model_name) {
            group.models.push(model_name.to_string());
        }
        group.keys.push(entry.litellm_params.api_key);
    }

    Ok(groups
        .into_iter()
        .map(|((kind, base_url), group)| ImportedProvider {
            name: provider_name(&kind, &base_url),
            kind,
            base_url,
            models: group.models,
            api_key: shared_key(&group.keys),
        })
        .collect())
}

#[derive(Default)]
struct Group {
    models: Vec<String>,
    keys: Vec<Option<String>>,
}

/// The one upstream key every entry of a group carries, or `None` when the
/// group is keyless or the entries disagree.
fn shared_key(keys: &[Option<String>]) -> Option<String> {
    let mut distinct = BTreeSet::new();
    for key in keys {
        let key = key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())?;
        distinct.insert(key.to_string());
    }
    if distinct.len() == 1 {
        distinct.into_iter().next()
    } else {
        None
    }
}

/// The provider kind implied by a LiteLLM `model` routing prefix.
fn kind_for(model: &str) -> &'static str {
    match model.split_once('/').map(|(prefix, _)| prefix) {
        Some("openai") => "openai",
        Some("anthropic") => "anthropic",
        _ => "custom",
    }
}

/// A stable provider name derived from the upstream kind and `base_url` host,
/// so re-importing the same config matches the existing provider.
fn provider_name(kind: &str, base_url: &str) -> String {
    let host = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| base_url.to_string());
    let sanitized: String = host
        .chars()
        .map(|ch| match ch {
            '.' | '-' => ch,
            _ if ch.is_ascii_alphanumeric() => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect();
    format!("litellm-{kind}-{sanitized}")
        .chars()
        .take(64)
        .collect()
}
