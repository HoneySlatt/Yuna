//! The small part of TOML the parity script's tracked files use, read by hand: comments, `key =
//! value` lines whose value is a string, a number or a list of strings, and `[[name]]` tables.
//! Anything else is refused, never guessed: a key given twice, a table the file may not hold, a
//! value of another type.

use std::collections::BTreeMap;

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A basic string, `\"` and `\\` its only escapes.
    Text(String),
    /// A number, `_` allowed between digits.
    Number(f64),
    /// A list of strings, on one line.
    List(Vec<String>),
}

/// A table: the file's top, or one `[[name]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Its name; none for the file's top.
    pub name: Option<String>,
    /// The line it starts at, from 1; 0 for the file's top.
    pub line: usize,
    /// Its keys, each with its value and its line.
    pub fields: BTreeMap<String, (Value, usize)>,
}

impl Table {
    /// Its name, for a message.
    fn place(&self) -> String {
        match &self.name {
            Some(name) => format!("the [[{name}]] at line {}", self.line),
            None => "the file".to_owned(),
        }
    }

    /// Refuses any key but `allowed`.
    pub fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self
            .fields
            .iter()
            .find(|(key, _)| !allowed.contains(&key.as_str()))
        {
            Some((key, (_, line))) => Err(format!(
                "line {line}: `{key}` is not a key of {}",
                self.place()
            )),
            None => Ok(()),
        }
    }

    /// The string `key`.
    pub fn text(&self, key: &str) -> Result<String, String> {
        match self.fields.get(key) {
            Some((Value::Text(text), _)) => Ok(text.clone()),
            Some((_, line)) => Err(format!("line {line}: `{key}` is not a string")),
            None => Err(format!("no `{key}` in {}", self.place())),
        }
    }

    /// The number `key`.
    pub fn number(&self, key: &str) -> Result<f64, String> {
        match self.fields.get(key) {
            Some((Value::Number(number), _)) => Ok(*number),
            Some((_, line)) => Err(format!("line {line}: `{key}` is not a number")),
            None => Err(format!("no `{key}` in {}", self.place())),
        }
    }

    /// The list of strings `key`.
    pub fn list(&self, key: &str) -> Result<Vec<String>, String> {
        match self.fields.get(key) {
            Some((Value::List(list), _)) => Ok(list.clone()),
            Some((_, line)) => Err(format!("line {line}: `{key}` is not a list")),
            None => Err(format!("no `{key}` in {}", self.place())),
        }
    }
}

/// Reads `text`: the file's top first, then each table, in order; tables other than `tables`
/// are refused.
pub fn parse(text: &str, tables: &[&str]) -> Result<Vec<Table>, String> {
    let mut read = vec![Table {
        name: None,
        line: 0,
        fields: BTreeMap::new(),
    }];
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix("[[")
            .and_then(|rest| rest.split_once("]]"))
            .filter(|(_, after)| after.trim().is_empty() || after.trim().starts_with('#'))
            .map(|(name, _)| name.trim())
        {
            if !tables.contains(&name) {
                return Err(format!("line {number}: no table [[{name}]] in this file"));
            }
            read.push(Table {
                name: Some(name.to_owned()),
                line: number,
                fields: BTreeMap::new(),
            });
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: expected `key = value`: {line:?}"))?;
        let key = key.trim().to_owned();
        let value = parse_value(value.trim()).map_err(|e| format!("line {number}: {e}"))?;
        let table = read.last_mut().expect("the file's top");
        if table.fields.insert(key.clone(), (value, number)).is_some() {
            return Err(format!("line {number}: `{key}` given twice"));
        }
    }
    Ok(read)
}

fn parse_value(text: &str) -> Result<Value, String> {
    if let Some(inner) = text.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or("a list must end on its line, with `]`")?;
        let mut items = Vec::new();
        let mut rest = inner.trim();
        while !rest.is_empty() {
            let (item, after) = parse_string(rest)?;
            items.push(item);
            rest = after.trim_start();
            match rest.strip_prefix(',') {
                Some(after) => rest = after.trim_start(),
                None if rest.is_empty() => {}
                None => return Err(format!("expected `,` or `]` before {rest:?}")),
            }
        }
        return Ok(Value::List(items));
    }
    if text.starts_with('"') {
        let (string, rest) = parse_string(text)?;
        let rest = rest.trim();
        if !rest.is_empty() && !rest.starts_with('#') {
            return Err(format!("unexpected {rest:?} after a string"));
        }
        return Ok(Value::Text(string));
    }
    let number = text.split('#').next().unwrap_or_default().trim();
    let digits: String = number.chars().filter(|c| *c != '_').collect();
    match digits.parse::<f64>() {
        Ok(value) if value.is_finite() && !digits.is_empty() => Ok(Value::Number(value)),
        _ => Err(format!(
            "not a string, a number or a list of strings: {text:?}"
        )),
    }
}

/// A basic string at the start of `text`, and what follows it.
fn parse_string(text: &str) -> Result<(String, &str), String> {
    let body = text
        .strip_prefix('"')
        .ok_or_else(|| format!("expected a string: {text:?}"))?;
    let mut string = String::new();
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Ok((string, &body[at + 1..])),
            '\\' => match chars.next() {
                Some((_, escaped @ ('"' | '\\'))) => string.push(escaped),
                _ => return Err("only `\\\"` and `\\\\` are escapes here".to_owned()),
            },
            c => string.push(c),
        }
    }
    Err(format!("a string that does not end: {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_reads_as_its_top_and_its_tables() {
        let text = "# A file.\nformat = 1\n\n[[move]]  # the first\nfigures = [\"a\", \"b \\\"c\\\"\"]\n\
                    reason = \"x\" # why\n[[move]]\nshare = 0.006\nfloor = 100_000\n";
        let tables = parse(text, &["move"]).unwrap();
        assert_eq!(tables.len(), 3);
        assert_eq!(tables[0].number("format"), Ok(1.0));
        assert_eq!(tables[1].list("figures").unwrap(), ["a", "b \"c\""]);
        assert_eq!(tables[1].text("reason").unwrap(), "x");
        assert_eq!(tables[2].number("share"), Ok(0.006));
        assert_eq!(tables[2].number("floor"), Ok(100_000.0));
        assert!(
            tables[2]
                .text("share")
                .unwrap_err()
                .contains("not a string")
        );
        assert!(
            tables[2]
                .text("reason")
                .unwrap_err()
                .contains("no `reason`")
        );
        assert!(tables[2].only(&["share"]).unwrap_err().contains("floor"));
        assert!(tables[2].only(&["share", "floor"]).is_ok());
    }

    #[test]
    fn what_the_files_may_not_hold_is_refused() {
        assert!(parse("[[threshold]]\n", &["move"]).is_err());
        assert!(parse("a = 1\na = 2\n", &[]).unwrap_err().contains("twice"));
        assert!(parse("a = [\"x\"\n", &[]).is_err());
        assert!(parse("a = [\"x\" \"y\"]\n", &[]).is_err());
        assert!(parse("a = \"x\n", &[]).is_err());
        assert!(parse("a = \"x\" y\n", &[]).is_err());
        assert!(parse("a = yes\n", &[]).is_err());
        assert!(parse("a = inf\n", &[]).is_err());
        assert!(parse("a\n", &[]).is_err());
        assert!(parse("", &[]).is_ok());
    }
}
