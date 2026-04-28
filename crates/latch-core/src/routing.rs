use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RouteTarget {
    NamedPool(String),
    BackendUrl(String),
}

impl RouteTarget {
    pub fn as_str(&self) -> &str {
        match self {
            RouteTarget::NamedPool(v) | RouteTarget::BackendUrl(v) => v,
        }
    }
}
