//! Small GDB/MI grammar parser. Keeps tuples, lists and C strings structured.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Tuple(Vec<(String, Value)>),
    List(Vec<Value>),
}
impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Tuple(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn text(&self) -> &str {
        if let Self::String(s) = self {
            s
        } else {
            ""
        }
    }
    pub fn field(&self, key: &str) -> &str {
        self.get(key).map(Self::text).unwrap_or("")
    }
    pub fn items(&self) -> &[Value] {
        if let Self::List(v) = self {
            v
        } else {
            &[]
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub token: Option<u64>,
    pub kind: char,
    pub class: String,
    pub data: Value,
}
struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }
    fn word(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || b"-_./".contains(&c))
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.input[start..self.pos]).into()
    }
    fn string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return Err("expected MI string".into());
        }
        self.pos += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                b'"' => return Ok(String::from_utf8_lossy(&out).into()),
                b'\\' => {
                    let c = self.peek().ok_or("unfinished MI escape")?;
                    self.pos += 1;
                    match c {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'v' => out.push(11),
                        b'0'..=b'7' => {
                            let mut n = (c - b'0') as u16;
                            for _ in 0..2 {
                                if let Some(d @ b'0'..=b'7') = self.peek() {
                                    n = n * 8 + (d - b'0') as u16;
                                    self.pos += 1;
                                } else {
                                    break;
                                }
                            }
                            out.push(n as u8);
                        }
                        _ => out.push(c),
                    }
                }
                _ => out.push(c),
            }
        }
        Err("unterminated MI string".into())
    }
    fn pair(&mut self) -> Result<(String, Value), String> {
        let key = self.word();
        if key.is_empty() || self.peek() != Some(b'=') {
            return Err("expected MI result".into());
        }
        self.pos += 1;
        Ok((key, self.value()?))
    }
    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'{') => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek() != Some(b'}') {
                    items.push(self.pair()?);
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if self.peek() != Some(b'}') {
                    return Err("unterminated MI tuple".into());
                }
                self.pos += 1;
                Ok(Value::Tuple(items))
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek() != Some(b']') {
                    if matches!(self.peek(), Some(b'"' | b'{' | b'[')) {
                        items.push(self.value()?);
                    } else {
                        items.push(Value::Tuple(vec![self.pair()?]));
                    }
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if self.peek() != Some(b']') {
                    return Err("unterminated MI list".into());
                }
                self.pos += 1;
                Ok(Value::List(items))
            }
            _ => Err("invalid MI value".into()),
        }
    }
}
pub fn parse(line: &str) -> Result<Option<Record>, String> {
    let line = line.trim();
    if line.is_empty() || line == "(gdb)" {
        return Ok(None);
    }
    let mut p = Parser {
        input: line.as_bytes(),
        pos: 0,
    };
    while p.peek().is_some_and(|b| b.is_ascii_digit()) {
        p.pos += 1;
    }
    let token = if p.pos == 0 {
        None
    } else {
        Some(line[..p.pos].parse().map_err(|_| "invalid token")?)
    };
    let kind = p.peek().ok_or("empty MI record")? as char;
    p.pos += 1;
    if "~@&".contains(kind) {
        let data = Value::String(p.string()?);
        return Ok(Some(Record {
            token,
            kind,
            class: String::new(),
            data,
        }));
    }
    if !"^*+=".contains(kind) {
        return Ok(None);
    }
    let class = p.word();
    let mut data = Vec::new();
    while p.peek() == Some(b',') {
        p.pos += 1;
        if p.peek() == Some(b'{') {
            if let Value::Tuple(fields) = p.value()? {
                data.extend(fields);
            }
        } else {
            data.push(p.pair()?);
        }
    }
    if p.pos != p.input.len() {
        return Err("trailing MI data".into());
    }
    Ok(Some(Record {
        token,
        kind,
        class,
        data: Value::Tuple(data),
    }))
}
pub fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_and_named_lists() {
        let r = parse(
            "17^done,stack=[frame={level=\"0\",func=\"main\",args=[{name=\"x\",value=\"2\"}]}]",
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.token, Some(17));
        assert_eq!(
            r.data.get("stack").unwrap().items()[0]
                .get("frame")
                .unwrap()
                .field("func"),
            "main"
        );
    }
    #[test]
    fn c_escapes() {
        let r = parse(r#"~"a\n\"quoted\"\t\344\270\255""#).unwrap().unwrap();
        assert_eq!(r.data.text(), "a\n\"quoted\"\t中");
        assert!(parse("1^done,x=\"broken").is_err());
    }
    #[test]
    fn async_and_prompt() {
        assert_eq!(
            parse("*stopped,reason=\"breakpoint-hit\"")
                .unwrap()
                .unwrap()
                .kind,
            '*'
        );
        assert_eq!(parse("(gdb) ").unwrap(), None);
    }
    #[test]
    fn download_progress() {
        let r = parse(r#"22+download,{section=".text",section-sent="7176"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(r.kind, '+');
        assert_eq!(r.data.field("section-sent"), "7176");
    }
}
