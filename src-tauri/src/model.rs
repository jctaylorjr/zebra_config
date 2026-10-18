use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "state", content = "detail")]
pub enum Status {
    Pending,
    Applying,
    Applied,
    Offline { attempt: u32, retry_in_secs: u64 },
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Printer {
    pub ip: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub status: Status,
}
