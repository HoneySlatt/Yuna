//! A JSON value and its writer, for the report's file: the standard library has none, and the
//! prototype takes no dependency but `ash`.

use std::fmt::Write;

pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    /// An exact integer; 64-bit values that must keep every bit, such as modifiers, go as text.
    Integer(i64),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}

impl Json {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn optional<T>(value: Option<T>, convert: impl FnOnce(T) -> Json) -> Self {
        value.map_or(Self::Null, convert)
    }

    /// The value, indented by two spaces a level, ending with a newline.
    pub fn to_pretty_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) if value.is_finite() => {
                let _ = write!(out, "{value}");
            }
            Self::Number(_) => out.push_str("null"),
            Self::Integer(value) => {
                let _ = write!(out, "{value}");
            }
            Self::Text(value) => write_string(out, value),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    item.write(out, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Self::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn escapes_strings_and_writes_nested_values() {
        let value = Json::Object(vec![
            ("text", Json::text("a \"b\"\n\u{1}")),
            (
                "list",
                Json::Array(vec![Json::Integer(-3), Json::Number(0.5), Json::Null]),
            ),
            ("empty", Json::Object(Vec::new())),
            ("nan", Json::Number(f64::NAN)),
        ]);
        assert_eq!(
            value.to_pretty_string(),
            "{\n  \"text\": \"a \\\"b\\\"\\n\\u0001\",\n  \"list\": [\n    -3,\n    0.5,\n    null\n  ],\n  \"empty\": {},\n  \"nan\": null\n}\n"
        );
    }
}
