use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::AppResult;

pub const MODEL_ROLES_SETTING_KEY: &str = "models:role_assignments";
pub const MAX_FALLBACK_MODELS: usize = 5;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ModelModality {
    Text,
    Image,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ModelCapabilities {
    pub input_modalities: Vec<ModelModality>,
    pub output_modalities: Vec<ModelModality>,
    pub embedding: bool,
}

impl ModelCapabilities {
    pub fn supports_text_generation(&self) -> bool {
        self.input_modalities.contains(&ModelModality::Text)
            && self.output_modalities.contains(&ModelModality::Text)
    }

    pub fn supports_image_understanding(&self) -> bool {
        self.input_modalities.contains(&ModelModality::Image)
            && self.output_modalities.contains(&ModelModality::Text)
    }

    pub fn supports_text_output(&self) -> bool {
        self.output_modalities.contains(&ModelModality::Text)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ModelRoleAssignments {
    pub main_model: Option<String>,
    pub image_model: Option<String>,
    pub summary_model: Option<String>,
    pub memory_model: Option<String>,
    pub speech_model: Option<String>,
    pub quick_model: Option<String>,
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub fallback_models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    Main,
    Image,
    Summary,
    Memory,
    Speech,
    Quick,
    Embedding,
}

impl ModelRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "主模型",
            Self::Image => "图片处理模型",
            Self::Summary => "对话总结模型",
            Self::Memory => "记忆更新模型",
            Self::Speech => "语音理解模型",
            Self::Quick => "快速模型",
            Self::Embedding => "嵌入模型",
        }
    }

    pub fn accepts(self, capabilities: &ModelCapabilities) -> bool {
        match self {
            Self::Main | Self::Summary | Self::Memory | Self::Quick => {
                capabilities.supports_text_generation()
            }
            Self::Image => capabilities.supports_image_understanding(),
            Self::Speech => capabilities.supports_text_output(),
            Self::Embedding => capabilities.embedding,
        }
    }
}

impl ModelRoleAssignments {
    pub fn normalize_fallback_models(&mut self) -> Result<(), String> {
        if self.fallback_models.len() > MAX_FALLBACK_MODELS {
            return Err(format!("备用模型最多配置 {MAX_FALLBACK_MODELS} 个"));
        }
        let mut seen = HashSet::new();
        self.fallback_models = self
            .fallback_models
            .drain(..)
            .map(|selection| selection.trim().to_string())
            .filter(|selection| !selection.is_empty() && seen.insert(selection.clone()))
            .collect();
        Ok(())
    }

    pub fn selections(&self) -> [(ModelRole, Option<&str>); 7] {
        [
            (ModelRole::Main, self.main_model.as_deref()),
            (ModelRole::Image, self.image_model.as_deref()),
            (ModelRole::Summary, self.summary_model.as_deref()),
            (ModelRole::Memory, self.memory_model.as_deref()),
            (ModelRole::Speech, self.speech_model.as_deref()),
            (ModelRole::Quick, self.quick_model.as_deref()),
            (ModelRole::Embedding, self.embedding_model.as_deref()),
        ]
    }

    pub fn clear_provider(&mut self, provider_id: &str) {
        let prefix = format!("{provider_id}/");
        for selection in [
            &mut self.main_model,
            &mut self.image_model,
            &mut self.summary_model,
            &mut self.memory_model,
            &mut self.speech_model,
            &mut self.quick_model,
            &mut self.embedding_model,
        ] {
            if selection
                .as_deref()
                .is_some_and(|value| value.starts_with(&prefix))
            {
                *selection = None;
            }
        }
        self.fallback_models
            .retain(|selection| !selection.starts_with(&prefix));
    }

    pub fn retain_valid_provider_models(&mut self, provider_id: &str, models: &[ModelDescriptor]) {
        let prefix = format!("{provider_id}/");
        for (role, selection) in [
            (ModelRole::Main, &mut self.main_model),
            (ModelRole::Image, &mut self.image_model),
            (ModelRole::Summary, &mut self.summary_model),
            (ModelRole::Memory, &mut self.memory_model),
            (ModelRole::Speech, &mut self.speech_model),
            (ModelRole::Quick, &mut self.quick_model),
            (ModelRole::Embedding, &mut self.embedding_model),
        ] {
            let should_clear = selection.as_deref().is_some_and(|value| {
                value.strip_prefix(&prefix).is_some_and(|model_id| {
                    models
                        .iter()
                        .find(|model| model.id == model_id)
                        .is_none_or(|model| !role.accepts(&model.capabilities))
                })
            });
            if should_clear {
                *selection = None;
            }
        }
        self.fallback_models.retain(|selection| {
            let Some(model_id) = selection.strip_prefix(&prefix) else {
                return true;
            };
            models
                .iter()
                .find(|model| model.id == model_id)
                .is_some_and(|model| model.capabilities.supports_text_generation())
        });
    }
}

pub async fn load_model_roles(db: &DbActorHandle) -> AppResult<ModelRoleAssignments> {
    let raw = db.get_setting(MODEL_ROLES_SETTING_KEY.to_string()).await?;
    Ok(raw
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default())
}

pub async fn save_model_roles(db: &DbActorHandle, roles: &ModelRoleAssignments) -> AppResult<()> {
    db.set_setting(
        MODEL_ROLES_SETTING_KEY.to_string(),
        serde_json::to_string(roles)?,
    )
    .await
}

pub fn parse_model_catalog(raw: Option<&str>, provider_kind: &str) -> Vec<ModelDescriptor> {
    let Some(Value::Array(entries)) = raw.and_then(|value| serde_json::from_str(value).ok()) else {
        return Vec::new();
    };

    let models = entries
        .into_iter()
        .filter_map(|entry| match entry {
            Value::String(id) => Some(ModelDescriptor {
                capabilities: infer_model_capabilities(&id, provider_kind),
                id,
            }),
            Value::Object(_) => serde_json::from_value(entry).ok(),
            _ => None,
        })
        .collect();
    normalize_model_catalog(models, provider_kind)
}

pub fn normalize_model_catalog(
    models: Vec<ModelDescriptor>,
    _provider_kind: &str,
) -> Vec<ModelDescriptor> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for mut model in models {
        model.id = model.id.trim().to_string();
        if model.id.is_empty() || !seen.insert(model.id.clone()) {
            continue;
        }
        normalize_modalities(&mut model.capabilities.input_modalities);
        normalize_modalities(&mut model.capabilities.output_modalities);
        normalized.push(model);
    }
    normalized
}

fn normalize_modalities(modalities: &mut Vec<ModelModality>) {
    let mut seen = HashSet::new();
    modalities.retain(|modality| seen.insert(*modality));
}

pub fn descriptor_from_api(provider_kind: &str, id: &str, metadata: &Value) -> ModelDescriptor {
    let mut capabilities = infer_model_capabilities(id, provider_kind);
    let architecture = metadata.get("architecture").unwrap_or(&Value::Null);
    let api_inputs = metadata
        .get("input_modalities")
        .or_else(|| architecture.get("input_modalities"))
        .and_then(Value::as_array)
        .map(|items| parse_api_modalities(items));
    let api_outputs = metadata
        .get("output_modalities")
        .or_else(|| architecture.get("output_modalities"))
        .and_then(Value::as_array)
        .map(|items| parse_api_modalities(items));
    if let Some(inputs) = api_inputs.filter(|items| !items.is_empty()) {
        capabilities.input_modalities = inputs;
    }
    if let Some(outputs) = api_outputs.filter(|items| !items.is_empty()) {
        capabilities.output_modalities = outputs;
    }
    let declared_capabilities = metadata
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if declared_capabilities
        .iter()
        .any(|value| value == "embedding")
    {
        capabilities.embedding = true;
    }
    if declared_capabilities.iter().any(|value| value == "vision")
        && !capabilities
            .input_modalities
            .contains(&ModelModality::Image)
    {
        capabilities.input_modalities.push(ModelModality::Image);
    }
    if capabilities.embedding
        && !declared_capabilities
            .iter()
            .any(|value| value == "completion")
    {
        capabilities.input_modalities.clear();
        capabilities.output_modalities.clear();
    }

    ModelDescriptor {
        id: id.to_string(),
        capabilities,
    }
}

fn parse_api_modalities(items: &[Value]) -> Vec<ModelModality> {
    items
        .iter()
        .filter_map(|item| match item.as_str()?.to_ascii_lowercase().as_str() {
            "text" => Some(ModelModality::Text),
            "image" => Some(ModelModality::Image),
            _ => None,
        })
        .collect()
}

pub fn infer_model_capabilities(id: &str, _provider_kind: &str) -> ModelCapabilities {
    let name = id.to_ascii_lowercase();
    if contains_any(
        &name,
        &[
            "embedding",
            "embed-",
            "embed_",
            "bge-",
            "bge_",
            "e5-",
            "e5_",
        ],
    ) {
        return ModelCapabilities {
            embedding: true,
            ..Default::default()
        };
    }

    if contains_any(
        &name,
        &[
            "gpt-image",
            "dall-e",
            "imagen",
            "stable-diffusion",
            "sdxl",
            "flux",
        ],
    ) {
        return ModelCapabilities {
            input_modalities: vec![ModelModality::Text, ModelModality::Image],
            output_modalities: vec![ModelModality::Image],
            embedding: false,
        };
    }

    if contains_any(&name, &["whisper", "speech-to-text", "transcribe"]) {
        return ModelCapabilities {
            input_modalities: Vec::new(),
            output_modalities: vec![ModelModality::Text],
            embedding: false,
        };
    }

    let mut input_modalities = vec![ModelModality::Text];
    if contains_any(
        &name,
        &[
            "gpt-4o",
            "gpt-4.1",
            "gpt-5",
            "o1",
            "o3",
            "o4",
            "vision",
            "llava",
            "qwen-vl",
            "qwen2-vl",
            "qwen2.5-vl",
            "minicpm-v",
            "gemma-3",
            "gemma3",
            "pixtral",
            "claude-3",
            "claude-sonnet-4",
            "claude-opus-4",
            "gemini",
        ],
    ) {
        input_modalities.push(ModelModality::Image);
    }
    ModelCapabilities {
        input_modalities,
        output_modalities: vec![ModelModality::Text],
        embedding: false,
    }
}

fn contains_any(value: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| value.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_string_catalog_gets_inferred_capabilities() {
        let models = parse_model_catalog(Some(r#"["gpt-4o","text-embedding-3-small"]"#), "openai");
        assert_eq!(models.len(), 2);
        assert!(models[0].capabilities.supports_image_understanding());
        assert!(models[1].capabilities.embedding);
    }

    #[test]
    fn api_metadata_overrides_inferred_modalities() {
        let descriptor = descriptor_from_api(
            "openai_compatible",
            "custom-model",
            &serde_json::json!({
                "architecture": {
                    "input_modalities": ["text", "image", "audio"],
                    "output_modalities": ["text"]
                }
            }),
        );
        assert!(descriptor.capabilities.supports_image_understanding());
        assert_eq!(descriptor.capabilities.input_modalities.len(), 2);
    }

    #[test]
    fn model_roles_enforce_capability_requirements() {
        let vision = infer_model_capabilities("gpt-4o", "openai");
        let embedding = infer_model_capabilities("bge-m3", "ollama");
        assert!(ModelRole::Image.accepts(&vision));
        assert!(!ModelRole::Embedding.accepts(&vision));
        assert!(ModelRole::Embedding.accepts(&embedding));
        assert!(!ModelRole::Main.accepts(&embedding));
    }

    #[test]
    fn removed_providers_and_models_clear_role_assignments() {
        let mut roles = ModelRoleAssignments {
            main_model: Some("openai/gpt-4o".to_string()),
            summary_model: Some("openai/gpt-4o-mini".to_string()),
            image_model: Some("ollama/llava".to_string()),
            ..Default::default()
        };
        roles.retain_valid_provider_models(
            "openai",
            &[ModelDescriptor {
                id: "gpt-4o".to_string(),
                capabilities: infer_model_capabilities("gpt-4o", "openai"),
            }],
        );
        assert_eq!(roles.main_model.as_deref(), Some("openai/gpt-4o"));
        assert!(roles.summary_model.is_none());

        roles.clear_provider("ollama");
        assert!(roles.image_model.is_none());
    }

    #[test]
    fn provider_changes_prune_invalid_fallback_models() {
        let mut roles = ModelRoleAssignments {
            fallback_models: vec![
                "openai/gpt-4o".into(),
                "openai/text-embedding-3-small".into(),
                "ollama/qwen".into(),
            ],
            ..Default::default()
        };
        roles.retain_valid_provider_models(
            "openai",
            &[
                ModelDescriptor {
                    id: "gpt-4o".into(),
                    capabilities: infer_model_capabilities("gpt-4o", "openai"),
                },
                ModelDescriptor {
                    id: "text-embedding-3-small".into(),
                    capabilities: infer_model_capabilities("text-embedding-3-small", "openai"),
                },
            ],
        );
        assert_eq!(roles.fallback_models, vec!["openai/gpt-4o", "ollama/qwen"]);
        roles.clear_provider("ollama");
        assert_eq!(roles.fallback_models, vec!["openai/gpt-4o"]);
    }

    #[test]
    fn fallback_models_are_bounded_trimmed_and_deduplicated() {
        let mut roles = ModelRoleAssignments {
            fallback_models: vec![
                " openai/gpt-4o ".into(),
                "openai/gpt-4o".into(),
                "".into(),
                "ollama/qwen".into(),
            ],
            ..Default::default()
        };
        roles.normalize_fallback_models().unwrap();
        assert_eq!(roles.fallback_models, vec!["openai/gpt-4o", "ollama/qwen"]);

        roles.fallback_models = (0..=MAX_FALLBACK_MODELS)
            .map(|index| format!("provider/model-{index}"))
            .collect();
        assert!(roles.normalize_fallback_models().is_err());
    }

    #[test]
    fn legacy_role_assignments_default_to_an_empty_fallback_chain() {
        let roles: ModelRoleAssignments = serde_json::from_str(
            r#"{
                "main_model":"openai/gpt-4o",
                "image_model":null,
                "summary_model":"openai/gpt-4o-mini",
                "memory_model":null,
                "speech_model":null,
                "quick_model":null,
                "embedding_model":null
            }"#,
        )
        .unwrap();

        assert_eq!(roles.main_model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(roles.summary_model.as_deref(), Some("openai/gpt-4o-mini"));
        assert!(roles.fallback_models.is_empty());
    }

    #[test]
    fn explicit_empty_capabilities_are_preserved() {
        let models = parse_model_catalog(Some(r#"[{"id":"gpt-4o","capabilities":{}}]"#), "openai");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].capabilities, ModelCapabilities::default());
    }

    #[test]
    fn capability_changes_clear_invalid_role_assignments() {
        let mut roles = ModelRoleAssignments {
            image_model: Some("openai/gpt-4o".to_string()),
            ..Default::default()
        };
        roles.retain_valid_provider_models(
            "openai",
            &[ModelDescriptor {
                id: "gpt-4o".to_string(),
                capabilities: ModelCapabilities {
                    input_modalities: vec![ModelModality::Text],
                    output_modalities: vec![ModelModality::Text],
                    embedding: false,
                },
            }],
        );
        assert!(roles.image_model.is_none());
    }
}
