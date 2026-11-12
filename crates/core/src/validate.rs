//! Value warnings (DESIGN.md §6.2, §8). Nothing here blocks: format checks only warn,
//! because key formats change and the agent may not know them.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::registry::Registry;
use crate::spec::{Field, SinkSpec, Widget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    /// Doesn't match the spec's `pattern`.
    Pattern,
    /// Doesn't start with any `x-prefix`.
    Prefix,
    /// Outside `minLength` / `maxLength`.
    Length,
    /// Fails `format`, or a key file without a PEM header.
    Format,
    /// File extension not in `x-accept`.
    Extension,
    /// Looks like a placeholder (`your-key-here`, `xxx`, `<…>`, very short secret).
    Placeholder,
    /// Surrounding whitespace was removed.
    Trimmed,
    /// Doesn't match the registry's usual format for this key.
    Registry,
    /// Contains `$`, which some dotenv loaders (Bun) expand.
    LoaderDependent,
    /// An echoed value that looks like a secret key.
    LooksLikeKey,
    /// Adding the destination to .gitignore failed.
    GitignoreFailed,
    /// Moving the dropped original to the Trash failed; it is still where it was.
    TrashFailed,
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("static regex"));
static URI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:\S+$").expect("static regex"));

/// The checks that apply to one field, compiled once.
#[derive(Debug, Clone)]
pub struct Rules {
    pattern: Option<Regex>,
    registry_pattern: Option<Regex>,
    prefixes: Vec<String>,
    min: Option<u64>,
    max: Option<u64>,
    format: Option<String>,
    accept: Vec<String>,
    secret: bool,
    dotenv: bool,
    echoed: bool,
}

impl Rules {
    pub fn for_field(field: &Field, registry: &Registry) -> Rules {
        let pattern = field.pattern.as_deref().and_then(|p| Regex::new(p).ok());
        // The spec's own pattern overrides the registry's.
        let registry_pattern = if field.pattern.is_none() {
            registry.lookup_for_field(field).and_then(|h| h.pattern.cloned())
        } else {
            None
        };
        Rules {
            pattern,
            registry_pattern,
            prefixes: field.prefix.clone().unwrap_or_default(),
            min: field.min_length,
            max: field.max_length,
            format: field.format.clone(),
            accept: field.accept.clone().unwrap_or_default().iter().map(|a| a.to_ascii_lowercase()).collect(),
            secret: field.write_only,
            dotenv: matches!(field.sink, Some(SinkSpec::Dotenv { .. })),
            echoed: field.echo,
        }
    }

    pub fn check_text(&self, value: &str, registry: &Registry) -> Vec<Warning> {
        let mut w = Vec::new();
        if value.is_empty() {
            return w;
        }
        if let Some(re) = &self.pattern
            && !re.is_match(value) {
                w.push(Warning::Pattern);
            }
        if let Some(re) = &self.registry_pattern
            && !re.is_match(value) {
                w.push(Warning::Registry);
            }
        if !self.prefixes.is_empty() && !self.prefixes.iter().any(|p| value.starts_with(p.as_str())) {
            w.push(Warning::Prefix);
        }
        let n = value.chars().count() as u64;
        if self.min.is_some_and(|m| n < m) || self.max.is_some_and(|m| n > m) {
            w.push(Warning::Length);
        }
        if let Some(fm) = &self.format {
            let ok = match fm.as_str() {
                "uuid" => uuid::Uuid::parse_str(value).is_ok(),
                "email" => EMAIL.is_match(value),
                "uri" => URI.is_match(value),
                _ => true,
            };
            if !ok {
                w.push(Warning::Format);
            }
        }
        if looks_like_placeholder(value, self.secret) {
            w.push(Warning::Placeholder);
        }
        if self.dotenv && value.contains('$') {
            w.push(Warning::LoaderDependent);
        }
        if self.echoed && registry.looks_like_key(value) {
            w.push(Warning::LooksLikeKey);
        }
        w
    }

    pub fn check_file(&self, file_name: &str, bytes: &[u8]) -> Vec<Warning> {
        let mut w = Vec::new();
        let lower = file_name.to_ascii_lowercase();
        if !self.accept.is_empty() && !self.accept.iter().any(|a| lower.ends_with(a.as_str())) {
            w.push(Warning::Extension);
        }
        let is_key_file = [".p8", ".pem", ".key"].iter().any(|e| lower.ends_with(e));
        if is_key_file && !contains(bytes, b"-----BEGIN ") {
            w.push(Warning::Format);
        }
        w
    }
}

/// Trim surrounding whitespace for single-line inputs; textareas (PEM blocks) are kept as is.
pub fn normalize(widget: Widget, raw: &str) -> (String, bool) {
    match widget {
        Widget::Textarea => (raw.to_string(), false),
        _ => {
            let t = raw.trim();
            (t.to_string(), t.len() != raw.len())
        }
    }
}

fn looks_like_placeholder(v: &str, secret: bool) -> bool {
    let l = v.to_ascii_lowercase();
    let n = v.chars().count();
    (secret && n < 6)
        || (l.contains("your") && ["here", "key", "token", "secret"].iter().any(|s| l.contains(s)))
        || l.contains("changeme")
        || l.contains("placeholder")
        || (l.starts_with('<') && l.ends_with('>'))
        || (n >= 3 && l.chars().all(|c| matches!(c, 'x' | '*' | '.' | '-' | '_')))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Field;

    fn field(json: &str) -> Field {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn registry_pattern_warns_but_spec_pattern_overrides() {
        let r = Registry::builtin();
        let f = field(r#"{"title":"K","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"OPENAI_API_KEY"}}"#);
        let rules = Rules::for_field(&f, r);
        assert_eq!(rules.check_text("sk-proj-abcdefgh", r), vec![]);
        assert_eq!(rules.check_text("pk-abcdefgh", r), vec![Warning::Registry]);

        let f2 = field(r#"{"title":"K","writeOnly":true,"pattern":"^pk-","x-sink":{"kind":"dotenv","path":".env","key":"OPENAI_API_KEY"}}"#);
        let rules2 = Rules::for_field(&f2, r);
        assert_eq!(rules2.check_text("pk-abcdefgh", r), vec![]);
    }

    #[test]
    fn format_prefix_length_placeholder_dollar() {
        let r = Registry::empty();
        let f = field(r#"{"title":"K","format":"uuid","x-prefix":["ab"],"minLength":40,"x-echo":true}"#);
        let rules = Rules::for_field(&f, &r);
        let got = rules.check_text("not-a-uuid", &r);
        assert_eq!(got, vec![Warning::Prefix, Warning::Length, Warning::Format]);

        let s = field(r#"{"title":"K","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"K"}}"#);
        let srules = Rules::for_field(&s, &r);
        assert_eq!(srules.check_text("abc", &r), vec![Warning::Placeholder]);
        assert_eq!(srules.check_text("your-api-key-here", &r), vec![Warning::Placeholder]);
        assert_eq!(srules.check_text("abcdef$HOME", &r), vec![Warning::LoaderDependent]);
    }

    #[test]
    fn echoed_key_lookalike_warns() {
        let r = Registry::builtin();
        let f = field(r#"{"title":"Issuer","x-echo":true}"#);
        let rules = Rules::for_field(&f, r);
        assert_eq!(rules.check_text("sk-ant-api03-aaaaaaaaaaaaaaaaaaaa", r), vec![Warning::LooksLikeKey]);
        assert_eq!(rules.check_text("57246542-96fe-1a63-e053-0824d011072a", r), vec![]);
    }

    #[test]
    fn file_checks() {
        let r = Registry::empty();
        let f = field(r#"{"title":"K","x-widget":"file","x-accept":[".p8"],"writeOnly":true,"x-sink":{"kind":"file","path":"k.p8"}}"#);
        let rules = Rules::for_field(&f, &r);
        assert_eq!(rules.check_file("AuthKey_X.p8", b"-----BEGIN PRIVATE KEY-----\nabc"), vec![]);
        assert_eq!(rules.check_file("AuthKey_X.P8", b"garbage"), vec![Warning::Format]);
        assert_eq!(rules.check_file("photo.png", b"x"), vec![Warning::Extension]);
    }

    #[test]
    fn normalize_trims_single_line_only() {
        assert_eq!(normalize(Widget::Text, "  sk-1\n"), ("sk-1".to_string(), true));
        assert_eq!(normalize(Widget::Text, "sk-1"), ("sk-1".to_string(), false));
        assert_eq!(normalize(Widget::Textarea, "a\nb\n"), ("a\nb\n".to_string(), false));
    }
}
