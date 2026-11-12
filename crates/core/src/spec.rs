//! The request spec: a JSON Schema 2020-12 subset with `x-` extensions (DESIGN.md §6).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::generate::GenerateSpec;
use crate::registry::Registry;
use crate::sinks::file::parse_mode;

pub const MAX_TITLE: usize = 80;
pub const MAX_PURPOSE: usize = 300;
pub const MAX_DESCRIPTION: usize = 1000;
pub const MAX_PATTERN: usize = 200;
pub const MAX_SHORT: usize = 200;
pub const MAX_URL: usize = 500;
pub const MAX_FIELDS: usize = 30;
pub const MAX_SPEC_BYTES: usize = 256 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
pub const MIN_TIMEOUT_SECS: u64 = 10;
pub const MAX_TIMEOUT_SECS: u64 = 3600;
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
pub const MAX_MAX_BYTES: u64 = 10 * 1024 * 1024;

static FIELD_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,63}$").expect("static regex"));
static ENV_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$").expect("static regex"));

const FORMATS: [&str; 3] = ["uuid", "email", "uri"];
const VERCEL_TARGETS: [&str; 3] = ["production", "preview", "development"];
const IGNORED_KEYWORDS: [&str; 2] = ["$comment", "$id"];

/// A passdrop request: what the agent needs and where each value goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Spec {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Window title. Max 80 characters.
    pub title: String,
    /// Why the agent needs these values. Shown in the window header. Max 300 characters.
    #[serde(rename = "x-purpose")]
    pub purpose: String,
    /// Shown under the title. Max 1000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Must be "object" when present.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    /// The fields, in display order.
    pub properties: IndexMap<String, Field>,
    /// Fields that must be filled before Submit is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Window timeout in seconds. Default 600, range 10..=3600.
    #[serde(rename = "x-timeout", default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    pub extra: BTreeMap<String, Value>,
}

/// One input in the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Field {
    /// Label. Max 80 characters.
    pub title: String,
    /// Help text. Plain text; https URLs become links. Max 1000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// "string" when present.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    /// Secret: masked input, never echoed, never in the result.
    #[serde(rename = "writeOnly", default, skip_serializing_if = "is_false")]
    pub write_only: bool,
    /// The first entry is shown as a greyed placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    /// Prefill. Not allowed with writeOnly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Renders a select.
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// Warn only. Rust regex syntax, unanchored as in JSON Schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Warn only.
    #[serde(rename = "minLength", default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// Warn only.
    #[serde(rename = "maxLength", default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// "uuid", "email" or "uri". Warn only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(rename = "x-widget", default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<Widget>,
    /// File extensions for a file field, e.g. [".p8"]. A mismatch warns.
    #[serde(rename = "x-accept", default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
    /// File size limit in bytes. Default 1 MiB, max 10 MiB. Blocks.
    #[serde(rename = "x-max-bytes", default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Expected prefixes. Warn only.
    #[serde(rename = "x-prefix", default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<Vec<String>>,
    /// Registry entry by env-var name, e.g. "OPENAI_API_KEY".
    #[serde(rename = "x-known", default, skip_serializing_if = "Option::is_none")]
    pub known: Option<String>,
    /// Where to get the value. https only. A registry URL wins over this one.
    #[serde(rename = "x-docs-url", default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// "hex:N", "base64:N" (N random bytes, 16..=128) or "uuid".
    #[serde(rename = "x-generate", default, skip_serializing_if = "Option::is_none")]
    pub generate: Option<String>,
    /// Destination.
    #[serde(rename = "x-sink", default, skip_serializing_if = "Option::is_none")]
    pub sink: Option<SinkSpec>,
    /// Return the value to the agent (visible in chat). Not allowed with writeOnly.
    #[serde(rename = "x-echo", default, skip_serializing_if = "is_false")]
    pub echo: bool,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    pub extra: BTreeMap<String, Value>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Widget {
    #[default]
    Text,
    Textarea,
    File,
}

/// Where a value is written. The list is fixed (DESIGN.md §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SinkSpec {
    Dotenv {
        path: String,
        key: String,
    },
    File {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir: Option<String>,
        /// "original" keeps the dropped file's name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Octal, owner bits only, e.g. "600". Default "600".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
    Keychain {
        service: String,
        account: String,
    },
    GhSecret {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        org: Option<String>,
    },
    VercelEnv {
        name: String,
        targets: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sensitive: Option<bool>,
    },
}

impl SinkSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            SinkSpec::Dotenv { .. } => "dotenv",
            SinkSpec::File { .. } => "file",
            SinkSpec::Keychain { .. } => "keychain",
            SinkSpec::GhSecret { .. } => "gh-secret",
            SinkSpec::VercelEnv { .. } => "vercel-env",
        }
    }
}

impl Spec {
    pub fn timeout_secs(&self) -> u64 {
        self.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    pub fn is_required(&self, name: &str) -> bool {
        self.required.iter().any(|r| r == name)
    }
}

impl Field {
    pub fn widget(&self) -> Widget {
        self.widget.unwrap_or_default()
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES)
    }

    pub fn generate_spec(&self) -> Option<GenerateSpec> {
        self.generate.as_deref().and_then(GenerateSpec::parse)
    }
}

/// The JSON Schema of the spec format (`passdrop schema`).
pub fn json_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Spec)).expect("schema serializes")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

impl Issue {
    pub fn new(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Issue { path: path.into(), code, message: message.into() }
    }
}

#[derive(Debug)]
pub struct SpecCheck {
    pub spec: Option<Spec>,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl SpecCheck {
    pub fn is_valid(&self) -> bool {
        self.spec.is_some() && self.errors.is_empty()
    }
}

/// Parse and check a spec. Destination guards need the filesystem and run later, in `run::plan`.
pub fn parse(json: &str, registry: &Registry) -> SpecCheck {
    if json.len() > MAX_SPEC_BYTES {
        return SpecCheck {
            spec: None,
            errors: vec![Issue::new("", "too_large", format!("spec is larger than {MAX_SPEC_BYTES} bytes"))],
            warnings: vec![],
        };
    }
    let spec: Spec = match serde_json::from_str(json) {
        Ok(s) => s,
        Err(e) => {
            return SpecCheck { spec: None, errors: vec![Issue::new("", "parse", e.to_string())], warnings: vec![] };
        }
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    check_spec(&spec, registry, &mut errors, &mut warnings);
    SpecCheck { spec: Some(spec), errors, warnings }
}

fn check_spec(spec: &Spec, registry: &Registry, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) {
    check_text("title", "title", &spec.title, MAX_TITLE, false, true, errors);
    check_text("x-purpose", "x-purpose", &spec.purpose, MAX_PURPOSE, false, true, errors);
    if let Some(d) = &spec.description {
        check_text("description", "description", d, MAX_DESCRIPTION, true, false, errors);
    }
    if let Some(t) = &spec.ty
        && t != "object" {
            errors.push(Issue::new("type", "unsupported_type", "top-level type must be \"object\""));
        }
    if spec.properties.is_empty() {
        errors.push(Issue::new("properties", "empty", "properties must contain at least one field"));
    }
    if spec.properties.len() > MAX_FIELDS {
        errors.push(Issue::new("properties", "too_many", format!("at most {MAX_FIELDS} fields are allowed")));
    }
    for r in &spec.required {
        if !spec.properties.contains_key(r) {
            errors.push(Issue::new("required", "unknown_required", format!("required names unknown field \"{r}\"")));
        }
    }
    if let Some(t) = spec.timeout
        && !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&t) {
            errors.push(Issue::new(
                "x-timeout",
                "invalid_timeout",
                format!("x-timeout must be between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS} seconds"),
            ));
        }
    check_extra("", &spec.extra, &["$schema"], errors, warnings);
    for (name, field) in &spec.properties {
        check_field(name, field, registry, errors, warnings);
    }
}

fn check_field(name: &str, f: &Field, registry: &Registry, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) {
    let p = format!("properties.{name}");
    let at = |k: &str| format!("{p}.{k}");
    if !FIELD_NAME.is_match(name) {
        errors.push(Issue::new(&p, "invalid_name", "field names must match ^[A-Za-z_][A-Za-z0-9_]{0,63}$"));
    }
    check_text(&at("title"), "title", &f.title, MAX_TITLE, false, true, errors);
    if let Some(d) = &f.description {
        check_text(&at("description"), "description", d, MAX_DESCRIPTION, true, false, errors);
    }
    if let Some(t) = &f.ty
        && t != "string" {
            errors.push(Issue::new(at("type"), "unsupported_type", "field type must be \"string\" in v1"));
        }
    let widget = f.widget();
    let generate = f.generate.as_deref();

    if let Some(values) = &f.enum_values {
        if widget != Widget::Text {
            errors.push(Issue::new(at("enum"), "conflicting_widget", "enum renders a select; it can't be combined with x-widget"));
        }
        if values.is_empty() {
            errors.push(Issue::new(at("enum"), "empty", "enum must not be empty"));
        }
        for v in values {
            check_text(&at("enum"), "enum value", v, MAX_SHORT, false, false, errors);
        }
    }
    if let Some(pat) = &f.pattern {
        if pat.chars().count() > MAX_PATTERN {
            errors.push(Issue::new(at("pattern"), "too_long", format!("pattern is longer than {MAX_PATTERN} characters")));
        } else if let Err(e) = Regex::new(pat) {
            errors.push(Issue::new(at("pattern"), "invalid_pattern", format!("pattern doesn't compile: {e}")));
        }
    }
    if let (Some(min), Some(max)) = (f.min_length, f.max_length)
        && min > max {
            errors.push(Issue::new(&p, "invalid_length", "minLength is greater than maxLength"));
        }
    if let Some(fm) = &f.format
        && !FORMATS.contains(&fm.as_str()) {
            warnings.push(Issue::new(at("format"), "unknown_format", format!("format \"{fm}\" is not checked")));
        }
    if let Some(ex) = &f.examples {
        for e in ex {
            check_text(&at("examples"), "example", e, MAX_SHORT, false, false, errors);
        }
    }
    if let Some(d) = &f.default {
        if f.write_only {
            errors.push(Issue::new(at("default"), "default_on_secret", "writeOnly fields can't have a default"));
        }
        if widget == Widget::File {
            errors.push(Issue::new(at("default"), "conflicting_widget", "file fields can't have a default"));
        }
        check_text(&at("default"), "default", d, MAX_SHORT, false, false, errors);
    }
    if let Some(g) = generate {
        if GenerateSpec::parse(g).is_none() {
            errors.push(Issue::new(
                at("x-generate"),
                "invalid_generate",
                "x-generate must be \"hex:N\", \"base64:N\" (N between 16 and 128) or \"uuid\"",
            ));
        }
        if widget != Widget::Text {
            errors.push(Issue::new(at("x-generate"), "conflicting_widget", "generated fields have no input widget"));
        }
        if f.default.is_some() || f.enum_values.is_some() {
            errors.push(Issue::new(at("x-generate"), "conflicting_generate", "x-generate can't be combined with default or enum"));
        }
        if f.echo {
            errors.push(Issue::new(at("x-generate"), "conflicting_generate", "generated values are never echoed"));
        }
    }
    if f.write_only && f.echo {
        errors.push(Issue::new(at("x-echo"), "echo_on_secret", "writeOnly fields can't be echoed"));
    }
    if f.echo && widget == Widget::File {
        errors.push(Issue::new(at("x-echo"), "conflicting_widget", "file fields can't be echoed"));
    }
    if f.sink.is_none() && !f.echo {
        errors.push(Issue::new(
            &p,
            "no_destination",
            "every field needs x-sink, x-echo: true, or both; nothing is echoed implicitly",
        ));
    }
    if let Some(url) = &f.docs_url
        && !is_safe_https_url(url) {
            errors.push(Issue::new(
                at("x-docs-url"),
                "invalid_url",
                format!("x-docs-url must be an https URL without spaces, at most {MAX_URL} characters"),
            ));
        }
    if let Some(acc) = &f.accept {
        if widget != Widget::File {
            errors.push(Issue::new(at("x-accept"), "conflicting_widget", "x-accept needs x-widget: \"file\""));
        }
        for a in acc {
            if !a.starts_with('.') || a.len() < 2 || a.len() > 20 || a.chars().any(|c| c.is_whitespace() || c == '/') {
                errors.push(Issue::new(at("x-accept"), "invalid_accept", "x-accept entries look like \".p8\""));
            }
        }
    }
    if let Some(mb) = f.max_bytes {
        if widget != Widget::File {
            errors.push(Issue::new(at("x-max-bytes"), "conflicting_widget", "x-max-bytes needs x-widget: \"file\""));
        }
        if mb == 0 || mb > MAX_MAX_BYTES {
            errors.push(Issue::new(at("x-max-bytes"), "invalid_max_bytes", format!("x-max-bytes must be between 1 and {MAX_MAX_BYTES}")));
        }
    }
    if let Some(prefixes) = &f.prefix
        && (prefixes.is_empty() || prefixes.iter().any(|x| x.is_empty() || x.chars().count() > 50)) {
            errors.push(Issue::new(at("x-prefix"), "invalid_prefix", "x-prefix entries must be 1 to 50 characters"));
        }
    if let Some(k) = &f.known
        && registry.get(k).is_none() {
            warnings.push(Issue::new(at("x-known"), "unknown_known", format!("\"{k}\" is not in the registry")));
        }
    if let Some(sink) = &f.sink {
        check_sink(&at("x-sink"), sink, widget, errors);
    }
    check_extra(&p, &f.extra, &[], errors, warnings);
}

fn check_sink(p: &str, sink: &SinkSpec, widget: Widget, errors: &mut Vec<Issue>) {
    match sink {
        SinkSpec::Dotenv { path, key } => {
            if path.trim().is_empty() {
                errors.push(Issue::new(p, "empty", "dotenv path must not be empty"));
            }
            if !ENV_NAME.is_match(key) {
                errors.push(Issue::new(p, "invalid_key", "dotenv key must match ^[A-Za-z_][A-Za-z0-9_]*$"));
            }
        }
        SinkSpec::File { dir, name, path, mode } => {
            match (dir, path) {
                (Some(_), Some(_)) | (None, None) => {
                    errors.push(Issue::new(p, "invalid_file_sink", "a file sink needs exactly one of \"dir\" or \"path\""));
                }
                (None, Some(_)) if name.is_some() => {
                    errors.push(Issue::new(p, "invalid_file_sink", "\"name\" only goes with \"dir\""));
                }
                (Some(_), None) => match name.as_deref() {
                    None => errors.push(Issue::new(p, "invalid_file_sink", "a \"dir\" file sink needs \"name\"")),
                    Some("original") if widget != Widget::File => errors.push(Issue::new(
                        p,
                        "invalid_file_sink",
                        "name \"original\" needs x-widget: \"file\"",
                    )),
                    Some("original") => {}
                    Some(n) if !crate::guards::is_valid_file_name(n) => {
                        errors.push(Issue::new(p, "invalid_file_sink", "name must be a plain file name"))
                    }
                    Some(_) => {}
                },
                _ => {}
            }
            if let Some(m) = mode
                && parse_mode(m).is_none() {
                    errors.push(Issue::new(
                        p,
                        "invalid_mode",
                        "mode must be octal with owner bits only and owner read, e.g. \"600\" or \"400\"",
                    ));
                }
        }
        SinkSpec::Keychain { service, account } => {
            if service.trim().is_empty() || account.trim().is_empty() {
                errors.push(Issue::new(p, "empty", "keychain service and account must not be empty"));
            }
        }
        SinkSpec::GhSecret { name, .. } => {
            if !ENV_NAME.is_match(name) || name.to_ascii_uppercase().starts_with("GITHUB_") {
                errors.push(Issue::new(
                    p,
                    "invalid_name",
                    "GitHub secret names use letters, digits and _, and can't start with GITHUB_",
                ));
            }
        }
        SinkSpec::VercelEnv { name, targets, .. } => {
            if !ENV_NAME.is_match(name) {
                errors.push(Issue::new(p, "invalid_name", "Vercel variable names must match ^[A-Za-z_][A-Za-z0-9_]*$"));
            }
            if targets.is_empty() || targets.iter().any(|t| !VERCEL_TARGETS.contains(&t.as_str())) {
                errors.push(Issue::new(p, "invalid_targets", "targets must be a non-empty list of production, preview, development"));
            }
        }
    }
}

fn check_extra(
    path: &str,
    extra: &BTreeMap<String, Value>,
    also_ignored: &[&str],
    errors: &mut Vec<Issue>,
    warnings: &mut Vec<Issue>,
) {
    for k in extra.keys() {
        let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
        if k.starts_with("x-") {
            errors.push(Issue::new(p, "unknown_extension", format!("unknown extension \"{k}\"")));
        } else if !IGNORED_KEYWORDS.contains(&k.as_str()) && !also_ignored.contains(&k.as_str()) {
            warnings.push(Issue::new(p, "unknown_keyword", format!("keyword \"{k}\" is ignored")));
        }
    }
}

fn check_text(path: &str, what: &str, s: &str, max: usize, allow_newline: bool, required: bool, errors: &mut Vec<Issue>) {
    if required && s.trim().is_empty() {
        errors.push(Issue::new(path, "empty", format!("{what} must not be empty")));
        return;
    }
    if s.chars().count() > max {
        errors.push(Issue::new(path, "too_long", format!("{what} is longer than {max} characters")));
    }
    if s.chars().any(|c| c.is_control() && !(allow_newline && c == '\n')) {
        errors.push(Issue::new(path, "control_chars", format!("{what} contains control characters")));
    }
}

/// https only, ASCII only, no whitespace, backslashes or credentials, length-capped.
pub fn is_safe_https_url(url: &str) -> bool {
    https_host(url).is_some()
}

/// The host a link really goes to, or None if the URL is not a plain https URL.
/// The window displays this host, so `https://openai.com@evil.example/` must not pass:
/// userinfo (`@`), backslashes and non-ASCII are rejected outright.
pub fn https_host(url: &str) -> Option<&str> {
    if url.len() > MAX_URL || !url.is_ascii() || url.chars().any(|c| c.is_ascii_whitespace() || c.is_ascii_control()) {
        return None;
    }
    if url.contains('\\') {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = port
        && (p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
            return None;
        }
    let labels_ok = host.split('.').all(|l| {
        !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !l.starts_with('-')
    });
    (host.contains('.') && labels_ok).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> &'static Registry {
        Registry::builtin()
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    fn minimal(field: &str) -> String {
        format!(r#"{{"title":"T","x-purpose":"P","type":"object","properties":{{"f":{field}}}}}"#)
    }

    #[test]
    fn minimal_echo_field_is_valid() {
        let c = parse(&minimal(r#"{"title":"Issuer","type":"string","x-echo":true}"#), reg());
        assert!(c.is_valid(), "{:?}", c.errors);
    }

    #[test]
    fn field_without_sink_or_echo_is_an_error() {
        let c = parse(&minimal(r#"{"title":"Key","type":"string"}"#), reg());
        assert_eq!(codes(&c.errors), vec!["no_destination"]);
    }

    #[test]
    fn secret_cannot_be_echoed_or_defaulted() {
        let c = parse(
            &minimal(r#"{"title":"Key","writeOnly":true,"x-echo":true,"default":"x","x-sink":{"kind":"dotenv","path":".env","key":"K"}}"#),
            reg(),
        );
        let got = codes(&c.errors);
        assert!(got.contains(&"echo_on_secret") && got.contains(&"default_on_secret"), "{got:?}");
    }

    #[test]
    fn unknown_extension_errors_unknown_keyword_warns() {
        let c = parse(&minimal(r#"{"title":"K","x-echo":true,"x-snk":1,"readOnly":true}"#), reg());
        assert_eq!(codes(&c.errors), vec!["unknown_extension"]);
        assert_eq!(codes(&c.warnings), vec!["unknown_keyword"]);
    }

    #[test]
    fn unknown_sink_field_is_a_parse_error() {
        let c = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"dotenv","path":".env","key":"K","extra":1}}"#), reg());
        assert_eq!(codes(&c.errors), vec!["parse"]);
    }

    #[test]
    fn unknown_sink_kind_is_a_parse_error() {
        let c = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"curl","url":"https://x"}}"#), reg());
        assert_eq!(codes(&c.errors), vec!["parse"]);
    }

    #[test]
    fn bad_pattern_and_generate_are_errors() {
        let c = parse(
            &minimal(r#"{"title":"K","pattern":"(?<=x)a","x-generate":"hex:8","x-sink":{"kind":"dotenv","path":".env","key":"K"}}"#),
            reg(),
        );
        let got = codes(&c.errors);
        assert!(got.contains(&"invalid_pattern") && got.contains(&"invalid_generate"), "{got:?}");
    }

    #[test]
    fn file_sink_shapes() {
        let ok = parse(
            &minimal(r#"{"title":"K","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"~/k/","name":"original","mode":"600"}}"#),
            reg(),
        );
        assert!(ok.is_valid(), "{:?}", ok.errors);
        let bad_mode = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"file","path":"k.txt","mode":"644"}}"#), reg());
        assert_eq!(codes(&bad_mode.errors), vec!["invalid_mode"]);
        let original_on_text = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"file","dir":"k","name":"original"}}"#), reg());
        assert_eq!(codes(&original_on_text.errors), vec!["invalid_file_sink"]);
        let traversal = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"file","dir":"k","name":"../x"}}"#), reg());
        assert_eq!(codes(&traversal.errors), vec!["invalid_file_sink"]);
    }

    #[test]
    fn docs_url_must_be_https() {
        let c = parse(&minimal(r#"{"title":"K","x-echo":true,"x-docs-url":"http://example.com"}"#), reg());
        assert_eq!(codes(&c.errors), vec!["invalid_url"]);
    }

    #[test]
    fn link_host_is_the_real_host() {
        assert_eq!(https_host("https://platform.openai.com/api-keys"), Some("platform.openai.com"));
        assert_eq!(https_host("https://example.com:8443/x?y#z"), Some("example.com"));
        assert_eq!(https_host("https://dashboard.clerk.com/~/api-keys"), Some("dashboard.clerk.com"));
        for bad in [
            "https://openai.com@evil.example/",
            "https://openai.com\\@evil.example/",
            "https://evil.example\\openai.com/",
            "https://xn--pple-43d.com/ok",
            "https://аpple.com/",
            "https://localhost/",
            "https://exa mple.com/",
            "https://example.com:/",
            "https://-bad.example.com/",
            "http://example.com/",
        ] {
            let ok = https_host(bad).is_some();
            let punycode = bad.contains("xn--");
            assert_eq!(ok, punycode, "{bad}");
        }
        let c = parse(&minimal(r#"{"title":"K","x-echo":true,"x-docs-url":"https://openai.com@evil.example/"}"#), reg());
        assert_eq!(codes(&c.errors), vec!["invalid_url"]);
    }

    #[test]
    fn every_registry_url_passes_the_link_check() {
        for e in Registry::builtin().entries() {
            if let Some(u) = &e.url {
                assert!(https_host(u).is_some(), "{}: {u}", e.name);
            }
        }
    }

    #[test]
    fn gh_secret_name_rules() {
        let c = parse(&minimal(r#"{"title":"K","x-sink":{"kind":"gh-secret","name":"GITHUB_TOKEN"}}"#), reg());
        assert_eq!(codes(&c.errors), vec!["invalid_name"]);
    }

    #[test]
    fn required_must_name_a_field_and_purpose_is_required() {
        let c = parse(r#"{"title":"T","x-purpose":" ","properties":{"f":{"title":"F","x-echo":true}},"required":["g"]}"#, reg());
        let got = codes(&c.errors);
        assert!(got.contains(&"empty") && got.contains(&"unknown_required"), "{got:?}");
    }

    #[test]
    fn schema_generates() {
        let s = json_schema();
        assert!(s.get("properties").is_some());
    }
}
