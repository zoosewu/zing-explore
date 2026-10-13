//! 命令語言：每行一個命令，`name 位置參數 --flag --key=value`。
//!
//! 參數內可用 `{變數}` 或 `{變數:參數}`，例如 `{sel}`、`{dest}`、`{date:%Y%m%d}`。
//! 先分詞再展開變數，所以含空白的路徑不會被拆開；
//! 單獨成為一個參數的多值變數（如 `{sel}`）會展開成多個參數。

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Positional(String),
    Flag(String, Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub name: String,
    pub args: Vec<Arg>,
}

impl Invocation {
    pub fn flag(&self, key: &str) -> Option<&str> {
        self.args.iter().find_map(|a| match a {
            Arg::Flag(k, Some(v)) if k == key => Some(v.as_str()),
            _ => None,
        })
    }

    pub fn has(&self, key: &str) -> bool {
        self.args.iter().any(|a| matches!(a, Arg::Flag(k, _) if k == key))
    }

    pub fn positional(&self) -> impl Iterator<Item = &str> {
        self.args.iter().filter_map(|a| match a {
            Arg::Positional(s) => Some(s.as_str()),
            _ => None,
        })
    }
}

/// 變數解析：回傳 `None` 表示未知變數；多值變數回傳多個字串。
pub trait Resolver {
    fn resolve(&mut self, name: &str, param: Option<&str>) -> Result<Option<Vec<String>>, String>;
}

impl<F> Resolver for F
where
    F: FnMut(&str, Option<&str>) -> Result<Option<Vec<String>>, String>,
{
    fn resolve(&mut self, name: &str, param: Option<&str>) -> Result<Option<Vec<String>>, String> {
        self(name, param)
    }
}

/// 解析整段命令腳本並展開變數。
pub fn parse(script: &str, vars: &mut dyn Resolver) -> Result<Vec<Invocation>, String> {
    let mut out = Vec::new();
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens = tokenize(line)?;
        let mut it = tokens.into_iter();
        let Some(name) = it.next() else { continue };
        let mut args = Vec::new();
        for tok in it {
            if let Some(rest) = tok.strip_prefix("--") {
                match rest.split_once('=') {
                    Some((k, v)) => {
                        let v = expand(v, vars)?.join(" ");
                        args.push(Arg::Flag(k.to_string(), Some(v)));
                    }
                    None => args.push(Arg::Flag(rest.to_string(), None)),
                }
            } else {
                args.extend(expand(&tok, vars)?.into_iter().map(Arg::Positional));
            }
        }
        out.push(Invocation { name: name.to_ascii_lowercase(), args });
    }
    Ok(out)
}

/// 分詞：空白分隔，支援雙引號（`""` 代表一個引號字元）。
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if in_quotes {
        return Err(format!("引號未關閉：{line}"));
    }
    if has_token {
        tokens.push(cur);
    }
    Ok(tokens)
}

/// 展開單一參數中的變數。整個參數就是一個變數時保留多值。
fn expand(tok: &str, vars: &mut dyn Resolver) -> Result<Vec<String>, String> {
    if let Some(inner) = tok.strip_prefix('{').and_then(|t| t.strip_suffix('}'))
        && !inner.contains(['{', '}']) {
            return lookup(inner, vars);
        }
    let mut out = String::new();
    let mut rest = tok;
    while let Some(start) = rest.find('{') {
        if rest[start..].starts_with("{{") {
            out.push_str(&rest[..start + 1]);
            rest = &rest[start + 2..];
            continue;
        }
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').ok_or_else(|| format!("大括號未關閉：{tok}"))? + start;
        out.push_str(&lookup(&rest[start + 1..end], vars)?.join(" "));
        rest = &rest[end + 1..];
    }
    out.push_str(&rest.replace("}}", "}"));
    Ok(vec![out])
}

fn lookup(inner: &str, vars: &mut dyn Resolver) -> Result<Vec<String>, String> {
    let (name, param) = match inner.split_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (inner, None),
    };
    vars.resolve(name, param)?.ok_or_else(|| format!("未知變數：{{{name}}}"))
}

/// 為外部程式組命令列時的參數加引號（Windows 規則）。
pub fn quote_arg(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str, _p: Option<&str>) -> Result<Option<Vec<String>>, String> {
        Ok(match name {
            "sel" => Some(vec![r"C:\a b\1.txt".into(), r"C:\a b\2.txt".into()]),
            "dest" => Some(vec![r"D:\out dir".into()]),
            "stem" => Some(vec!["photo".into()]),
            _ => None,
        })
    }

    #[test]
    fn parse_and_expand() {
        let inv = parse("copy --move --to={dest} --when-exists=rename", &mut vars).unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].name, "copy");
        assert!(inv[0].has("move"));
        assert_eq!(inv[0].flag("to"), Some(r"D:\out dir"));
        assert_eq!(inv[0].flag("when-exists"), Some("rename"));

        let inv = parse("run code {sel}\n# 註解\nnav.goto \"C:\\Program Files\"", &mut vars).unwrap();
        assert_eq!(inv.len(), 2);
        assert_eq!(inv[0].positional().collect::<Vec<_>>(), ["code", r"C:\a b\1.txt", r"C:\a b\2.txt"]);
        assert_eq!(inv[1].positional().next(), Some(r"C:\Program Files"));

        let inv = parse("rename --to={stem}_v2.{{x}}", &mut vars).unwrap();
        assert_eq!(inv[0].flag("to"), Some("photo_v2.{x}"));

        assert!(parse("copy {nope}", &mut vars).is_err());
        assert!(parse("copy \"open", &mut vars).is_err());
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_arg("abc"), "abc");
        assert_eq!(quote_arg(r"C:\a b\"), r#""C:\a b\\""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }
}
