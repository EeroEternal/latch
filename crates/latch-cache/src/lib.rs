use latch_core::{Message, PromptCacheProvider};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheTaggedMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl From<Message> for CacheTaggedMessage {
    fn from(value: Message) -> Self {
        Self {
            role: value.role,
            content: value.content,
            cache_control: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptCachePlan {
    pub provider: PromptCacheProvider,
    pub tagged_indexes: Vec<usize>,
}

pub fn plan_prompt_cache(
    messages: &[Message],
    provider: PromptCacheProvider,
    cache_roles: &[String],
    min_content_chars: usize,
) -> PromptCachePlan {
    match provider {
        PromptCacheProvider::Anthropic => {
            let tagged_indexes = messages
                .iter()
                .enumerate()
                .filter_map(|(idx, m)| {
                    if !cache_roles.contains(&m.role) {
                        return None;
                    }
                    if m.content.chars().count() < min_content_chars {
                        return None;
                    }
                    Some(idx)
                })
                .collect();

            PromptCachePlan {
                provider,
                tagged_indexes,
            }
        }
        PromptCacheProvider::OpenAiCompatible | PromptCacheProvider::None => PromptCachePlan {
            provider,
            tagged_indexes: Vec::new(),
        },
    }
}

/// Backward-compatible convenience function with default parameters.
pub fn plan_prompt_cache_default(
    messages: &[Message],
    provider: PromptCacheProvider,
) -> PromptCachePlan {
    plan_prompt_cache(messages, provider, &["system".to_string()], 0)
}

pub fn apply_prompt_cache_plan(
    messages: &[Message],
    plan: &PromptCachePlan,
) -> Vec<CacheTaggedMessage> {
    let mut out: Vec<CacheTaggedMessage> = messages.iter().cloned().map(Into::into).collect();
    for idx in &plan.tagged_indexes {
        if let Some(msg) = out.get_mut(*idx) {
            msg.cache_control = Some(CacheControl {
                kind: "ephemeral".to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{apply_prompt_cache_plan, plan_prompt_cache, plan_prompt_cache_default};
    use latch_core::{Message, PromptCacheProvider};

    fn msg(role: impl Into<String>, content: impl Into<String>) -> Message {
        Message::new(role, content)
    }

    #[test]
    fn anthropic_tags_system_messages_with_sufficient_length() {
        let messages = vec![
            msg("system", "a".repeat(100)), // Long enough
            msg("user", "hello"),
            msg("assistant", "hi"),
            msg("system", "short"), // Too short
        ];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic, &["system".to_string()], 100);
        assert_eq!(plan.tagged_indexes, vec![0]);
    }

    #[test]
    fn anthropic_tags_multiple_roles() {
        let messages = vec![
            msg("system", "a".repeat(100)),
            msg("user", "b".repeat(100)),
        ];
        let cache_roles = vec!["system".to_string(), "user".to_string()];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic, &cache_roles, 100);
        assert_eq!(plan.tagged_indexes, vec![0, 1]);
    }

    #[test]
    fn openai_compatible_has_no_tags() {
        let messages = vec![msg("system", "policy"), msg("user", "hello")];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::OpenAiCompatible, &["system".to_string()], 0);
        assert!(plan.tagged_indexes.is_empty());
    }

    #[test]
    fn apply_sets_ephemeral_marker_only_for_planned_indexes() {
        let messages = vec![msg("system", "a".repeat(100)), msg("user", "hello")];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic, &["system".to_string()], 100);
        let out = apply_prompt_cache_plan(&messages, &plan);

        assert_eq!(
            out[0]
                .cache_control
                .as_ref()
                .map(|cc| cc.kind.as_str())
                .unwrap_or(""),
            "ephemeral"
        );
        assert!(out[1].cache_control.is_none());
    }

    #[test]
    fn plan_prompt_cache_default_works() {
        let messages = vec![
            msg("system", "a".repeat(100)),
            msg("user", "hello"),
        ];
        let plan = plan_prompt_cache_default(&messages, PromptCacheProvider::Anthropic);
        assert_eq!(plan.provider, PromptCacheProvider::Anthropic);
        assert_eq!(plan.tagged_indexes, vec![0]);
    }
}
