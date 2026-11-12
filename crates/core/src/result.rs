//! The output contract (DESIGN.md §5, §9.1): one JSON line on stdout, metadata only.

use indexmap::IndexMap;
use serde::Serialize;

use crate::spec::Issue;
use crate::validate::Warning;

pub const NEXT_OK: &str = "Values are in place. Do not read or print them; use them by reference.";
pub const NEXT_FAILED: &str =
    "Some values were not written; see each field's error. Do not ask for them in chat; fix the spec or ask the user to run the request again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Valid,
    Partial,
    Failed,
    Cancelled,
    Timeout,
    NoGui,
    InvalidSpec,
    Usage,
    Error,
}

impl Status {
    pub fn exit_code(self) -> i32 {
        match self {
            Status::Ok | Status::Valid => 0,
            Status::Error => 1,
            Status::InvalidSpec | Status::Usage => 2,
            Status::Cancelled => 10,
            Status::Timeout => 11,
            Status::NoGui => 12,
            Status::Partial => 20,
            Status::Failed => 21,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Created,
    Updated,
    Unchanged,
    Kept,
    Generated,
    Echoed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldResult {
    pub action: Action,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Only for `x-echo` fields, which can never be `writeOnly`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl FieldResult {
    pub fn new(action: Action) -> FieldResult {
        FieldResult {
            action,
            reference: None,
            filename: None,
            verified: None,
            original: None,
            warnings: Vec::new(),
            error: None,
            message: None,
            value: None,
        }
    }

    pub fn failed(code: &'static str, message: impl Into<String>) -> FieldResult {
        let mut r = FieldResult::new(Action::Failed);
        r.error = Some(code);
        r.message = Some(message.into());
        r
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub status: Status,
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub fields: IndexMap<String, FieldResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Issue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<&'static str>,
}

impl RunResult {
    pub fn status_only(status: Status, message: impl Into<String>) -> RunResult {
        RunResult {
            status,
            fields: IndexMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            message: Some(message.into()),
            next: None,
        }
    }

    pub fn invalid_spec(errors: Vec<Issue>, warnings: Vec<Issue>) -> RunResult {
        RunResult { status: Status::InvalidSpec, fields: IndexMap::new(), errors, warnings, message: None, next: None }
    }

    pub fn exit_code(&self) -> i32 {
        self.status.exit_code()
    }

    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("result serializes")
    }
}
