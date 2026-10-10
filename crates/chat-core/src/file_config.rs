//! Optional `rustchat.yaml` deployment config.
//!
//! `rustchat.yaml` is a read-only, global base layer for declarative settings
//! (global providers, defaults, ...). It is meant to be baked into an image or
//! mounted from a ConfigMap, so it must never contain secrets — provider keys
//! keep resolving from the DB/env via `SecretCipher`. Environment variables and
//! runtime/DB state always override it.
//!
//! The path comes from `RUSTCHAT_CONFIG` and defaults to
//! `/etc/rustchat/rustchat.yaml`; a missing file is not an error.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ChatError;

/// Default on-disk location, overridable with `RUSTCHAT_CONFIG`.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/rustchat/rustchat.yaml";

/// One globally-declared provider and the models it advertises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileProvider {
    /// Unique name referenced by `defaults` / `title_model`.
    pub name: String,
    /// `openai` | `anthropic` | `custom`.
    pub kind: String,
    pub base_url: String,
    /// Models the provider advertises. Empty means "discover at runtime".
    #[serde(default)]
    pub models: Vec<String>,
}

/// A `provider` + `model` pair, as used by `defaults` and `title_model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileModelRef {
    pub provider: String,
    pub model: String,
}

/// The parsed contents of `rustchat.yaml`, empty when no file is present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default)]
    pub providers: Vec<FileProvider>,
    /// Base `provider` + `model` used when nothing else names one.
    #[serde(default)]
    pub defaults: Option<FileModelRef>,
    /// Provider + model used to auto-generate conversation titles.
    #[serde(default)]
    pub title_model: Option<FileModelRef>,
}

impl FileConfig {
    /// Load the config file named by `RUSTCHAT_CONFIG`, falling back to
    /// [`DEFAULT_CONFIG_PATH`]. A missing file yields an empty config.
    pub fn load() -> Result<Self, ChatError> {
        let path = env::var("RUSTCHAT_CONFIG")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        Self::load_from(Path::new(&path))
    }

    /// Load and validate the config file at `path`.
    pub fn load_from(path: &Path) -> Result<Self, ChatError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            // A missing file is fine: the file is an optional base layer.
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(ChatError::Config(format!(
                    "failed to read config file {}: {err}",
                    path.display()
                )));
            }
        };
        Self::parse(&text, path)
    }

    /// Parse, interpolate `${ENV}` values in, and validate a config document.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ChatError> {
        let mut value: serde_yaml::Value = serde_yaml::from_str(text)
            .map_err(|err| ChatError::Config(format!("{}: invalid YAML: {err}", path.display())))?;

        // An empty document parses as null; treat it as an empty config.
        if value.is_null() {
            return Ok(Self::default());
        }

        interpolate_env(&mut value, path)?;

        let config: Self = serde_yaml::from_value(value)
            .map_err(|err| ChatError::Config(format!("{}: {err}", path.display())))?;
        config.validate(path)?;
        Ok(config)
    }

    /// Find a declared provider by name.
    pub fn provider(&self, name: &str) -> Option<&FileProvider> {
        self.providers.iter().find(|provider| provider.name == name)
    }

    /// Reject duplicate/blank providers and references to unknown providers or
    /// models, so a bad config fails fast at boot instead of at first request.
    fn validate(&self, path: &Path) -> Result<(), ChatError> {
        let mut seen: HashSet<&str> = HashSet::new();
        for provider in &self.providers {
            if provider.name.trim().is_empty() {
                return Err(ChatError::Config(format!(
                    "{}: provider name must not be empty",
                    path.display()
                )));
            }
            if !seen.insert(provider.name.as_str()) {
                return Err(ChatError::Config(format!(
                    "{}: duplicate provider `{}`",
                    path.display(),
                    provider.name
                )));
            }
            if provider.kind.trim().is_empty() {
                return Err(ChatError::Config(format!(
                    "{}: provider `{}` has an empty kind",
                    path.display(),
                    provider.name
                )));
            }
            if provider.base_url.trim().is_empty() {
                return Err(ChatError::Config(format!(
                    "{}: provider `{}` has an empty base_url",
                    path.display(),
                    provider.name
                )));
            }
        }

        self.check_reference(self.defaults.as_ref(), "defaults", path)?;
        self.check_reference(self.title_model.as_ref(), "title_model", path)?;
        Ok(())
    }

    fn check_reference(
        &self,
        reference: Option<&FileModelRef>,
        field: &str,
        path: &Path,
    ) -> Result<(), ChatError> {
        let Some(reference) = reference else {
            return Ok(());
        };
        let Some(provider) = self.provider(&reference.provider) else {
            return Err(ChatError::Config(format!(
                "{}: `{field}` references unknown provider `{}`",
                path.display(),
                reference.provider
            )));
        };
        // Only enforce model membership when the provider declares a fixed list.
        if !provider.models.is_empty() && !provider.models.iter().any(|m| m == &reference.model) {
            return Err(ChatError::Config(format!(
                "{}: `{field}` references unknown model `{}` for provider `{}`",
                path.display(),
                reference.model,
                reference.provider
            )));
        }
        Ok(())
    }
}

/// Recursively expand `${ENV}` / `${ENV:-default}` in every string scalar. Keys
/// are left untouched and the file is expected to hold no secrets.
fn interpolate_env(value: &mut serde_yaml::Value, path: &Path) -> Result<(), ChatError> {
    match value {
        serde_yaml::Value::String(text) => *text = expand_env(text, path)?,
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                interpolate_env(item, path)?;
            }
        }
        serde_yaml::Value::Mapping(entries) => {
            for (_key, entry) in entries.iter_mut() {
                interpolate_env(entry, path)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Expand `${NAME}` and `${NAME:-default}` references against the environment.
/// An unset variable without a default is a boot error.
fn expand_env(input: &str, path: &Path) -> Result<String, ChatError> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // Unmatched `${`; keep it literal.
            out.push_str("${");
            rest = after;
            continue;
        };
        let inner = &after[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name.trim(), Some(default)),
            None => (inner.trim(), None),
        };
        if name.is_empty() {
            return Err(ChatError::Config(format!(
                "{}: empty `${{}}` interpolation",
                path.display()
            )));
        }
        match env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) => match default {
                Some(default) => out.push_str(default),
                None => {
                    return Err(ChatError::Config(format!(
                        "{}: environment variable `{name}` is not set (used in `{input}`)",
                        path.display()
                    )));
                }
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
