use latch_core::message::Message;

/// Content profile extracted from messages
#[derive(Debug, Default)]
pub struct ContentProfile {
    pub has_code: bool,
    pub has_architecture: bool,
    pub has_images: bool,
    pub is_simple: bool,
    pub has_failure_retry: bool,
    pub chinese_ratio: f32,
}

/// Analyze messages for content features
pub fn analyze_content(messages: &[Message]) -> ContentProfile {
    let mut profile = ContentProfile::default();

    // Look at last 3 messages (or fewer if less available)
    let start_idx = messages.len().saturating_sub(3);
    let recent_messages: Vec<&Message> = messages.iter().skip(start_idx).collect();

    for msg in &recent_messages {
        let content_lower = msg.content.to_lowercase();

        // Detect code
        if !profile.has_code {
            profile.has_code = msg.content.contains("```")
                || contains_any(&content_lower, &[
                    "rust", "python", "javascript", "go", "java", "c++", "typescript",
                    "fn ", "def ", "function ", "impl ", "class ", "struct ",
                ])
                || has_indented_lines(&msg.content);
        }

        // Detect architecture/design
        if !profile.has_architecture {
            profile.has_architecture = contains_any(&content_lower, &[
                "design", "architecture", "tradeoff", "refactor", "migrate",
                "system design", "scalability", "pattern",
            ]);
        }

        // Detect images
        if !profile.has_images {
            profile.has_images = msg.content.contains("data:image/")
                || msg.content.contains("base64")
                || contains_any(&content_lower, &[".png", ".jpg", ".jpeg", ".gif", ".webp"]);
        }

        // Detect failure/retry
        if !profile.has_failure_retry {
            profile.has_failure_retry = contains_any(&content_lower, &[
                "fix this error", "still broken", "try again", "not working",
                "error:", "exception:", "failed",
            ]);
        }
    }

    // Check if simple (single line, < 50 chars)
    if let Some(last_user) = messages.iter().rev().find(|m| m.role == "user") {
        let line_count = last_user.content.lines().count();
        profile.is_simple = line_count <= 1 && last_user.content.chars().count() < 50;
    }

    // Calculate Chinese character ratio
    let total_chars: usize = recent_messages.iter().map(|m| m.content.chars().count()).sum();
    if total_chars > 0 {
        let chinese_chars: usize = recent_messages
            .iter()
            .flat_map(|m| m.content.chars())
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .count();
        profile.chinese_ratio = chinese_chars as f32 / total_chars as f32;
    }

    profile
}

/// Check if text contains any of the keywords
pub fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|k| text.contains(k))
}

/// Check if content has indented lines (potential code)
pub fn has_indented_lines(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("    ") || line.starts_with("\t"))
}
