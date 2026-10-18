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
    pub profile_id: Option<String>,
    pub status: Status,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub models: Vec<String>, // substrings matched against the printer's model, e.g. "ZD411"; empty = any
    pub commands: Vec<String>, // SGD/ZPL lines to send
}
