use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }

    /// Check if this message contains a tool_use content block
    /// This is a lightweight check that looks for the "type":"tool_use" pattern in content
    pub fn is_tool_call(&self) -> bool {
        self.role == "assistant" && self.content.contains("\"type\":\"tool_use\"")
    }

    /// Check if this message contains a tool_result content block
    /// This is a lightweight check that looks for the "type":"tool_result" pattern in content
    pub fn is_tool_result(&self) -> bool {
        self.role == "user" && self.content.contains("\"type\":\"tool_result\"")
    }
}
