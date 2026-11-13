//! 极简 JSON 读取：面板读后端接口和 stack 的状态文件用。
//! 不引 serde_json：给 deploy-dash 加依赖有让已编好的依赖跟着重编的风险。
#![allow(dead_code)]

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

static NULL: Json = Json::Null;

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let text = text.trim_start_matches('\u{feff}');
        let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("第 {} 字节之后还有多余的内容", p.i));
        }
        Ok(v)
    }

    /// 取对象里的一格；没有这一格（或者不是对象）就是 Null。
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn at(&self, path: &[&str]) -> &Json {
        path.iter().fold(self, |v, k| v.get(k))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// 字符串原样；数字、布尔转成文字；其余是空串。
    pub fn text(&self) -> String {
        match self {
            Json::Str(s) => s.clone(),
            Json::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Json::Num(n) => n.to_string(),
            Json::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    pub fn f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            Json::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn i64(&self) -> Option<i64> {
        self.f64().map(|v| v as i64)
    }

    pub fn u64(&self) -> Option<u64> {
        self.f64().filter(|v| *v >= 0.0).map(|v| v as u64)
    }

    pub fn bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            _ => &[],
        }
    }

    pub fn entries(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(items) => items,
            _ => &[],
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: u32,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, lit: &str) -> Result<(), String> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(format!("第 {} 字节处应为 {lit}", self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > 256 {
            return Err("嵌套太深".into());
        }
        let v = match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.expect("true").map(|_| Json::Bool(true)),
            Some(b'f') => self.expect("false").map(|_| Json::Bool(false)),
            Some(b'n') => self.expect("null").map(|_| Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("第 {} 字节处是意外的字符 {:?}", self.i, c as char)),
            None => Err("内容提前结束".into()),
        };
        self.depth -= 1;
        v
    }

    fn object(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(items));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(":")?;
            self.ws();
            let v = self.value()?;
            items.push((key, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(items));
                }
                _ => return Err(format!("第 {} 字节处对象没有正确结束", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("第 {} 字节处数组没有正确结束", self.i)),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let end = self.i + 4;
        let digits = self.s.get(self.i..end).ok_or("\\u 后面不足 4 位")?;
        let text = std::str::from_utf8(digits).map_err(|_| "\\u 后面不是十六进制")?;
        let v = u32::from_str_radix(text, 16).map_err(|_| "\\u 后面不是十六进制")?;
        self.i = end;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return Err(format!("第 {} 字节处应为字符串", self.i));
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err("字符串没有结束".into()) };
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("转义没有结束".into()) };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        }
                        other => return Err(format!("认不出的转义 \\{}", other as char)),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "数字不是 ASCII")?;
        text.parse::<f64>().map(Json::Num).map_err(|_| format!("认不出的数字 {text}"))
    }
}

/// 拼 JSON 字符串字面量（发请求时用）。
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backend_shapes() {
        let v = Json::parse(r#"{"paused":true,"rows":[{"dbnum":7352,"state":"running","phase":"解析"}],"x":-1.5e2,"s":"a\"b\u4e2d"}"#).unwrap();
        assert_eq!(v.get("paused").bool(), Some(true));
        assert_eq!(v.get("rows").arr()[0].get("dbnum").u64(), Some(7352));
        assert_eq!(v.get("rows").arr()[0].get("phase").str(), Some("解析"));
        assert_eq!(v.get("x").f64(), Some(-150.0));
        assert_eq!(v.get("s").str(), Some("a\"b中"));
        assert!(v.get("missing").is_null());
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
}
