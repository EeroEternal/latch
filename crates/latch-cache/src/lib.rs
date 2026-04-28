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

pub fn plan_prompt_cache(messages: &[Message], provider: PromptCacheProvider) -> PromptCachePlan {
    match provider {
        PromptCacheProvider::Anthropic => PromptCachePlan {
            provider,
            tagged_indexes: messages
                .iter()
                .enumerate()
                .filter_map(|(idx, m)| (m.role == "system").then_some(idx))
                .collect(),
        },
        PromptCacheProvider::OpenAiCompatible | PromptCacheProvider::None => PromptCachePlan {
            provider,
            tagged_indexes: Vec::new(),
        },
    }
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
    use super::{apply_prompt_cache_plan, plan_prompt_cache};
    use latch_core::{Message, PromptCacheProvider};

    fn msg(role: &str, content: &str) -> Message {
        Message::new(role, content)
    }

    #[test]
    fn anthropic_tags_system_messages() {
        let messages = vec![
            msg("system", "policy"),
            msg("user", "hello"),
            msg("assistant", "hi"),
            msg("system", "persona"),
        ];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic);
        assert_eq!(plan.tagged_indexes, vec![0, 3]);
    }

    #[test]
    fn openai_compatible_has_no_tags() {
        let messages = vec![msg("system", "policy"), msg("user", "hello")];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::OpenAiCompatible);
        assert!(plan.tagged_indexes.is_empty());
    }

    #[test]
    fn apply_sets_ephemeral_marker_only_for_planned_indexes() {
        let messages = vec![msg("system", "policy"), msg("user", "hello")];
        let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic);
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
}
