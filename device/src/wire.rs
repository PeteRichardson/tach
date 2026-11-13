//! The reply line from tach-sender.py, read without an allocator (docs/device-v2.md, "Wire").
//!
//! The reply is one flat JSON object of numbers and booleans. This reader takes exactly that
//! shape: no nesting, and a string value is skipped. A key it does not know is skipped too, so the
//! Mac can add keys without a firmware change. A value that does not parse as its key's type is
//! treated as missing, the same as a key the Mac left out.

/// The protocol version this firmware reads. A reply with another `v` is not used.
pub const VERSION: u32 = 2;

/// One reply. Every field is optional: the Mac leaves out a value it does not know.
/// Stamps (`t`, `tc`, `tg`, `tm`) are Mac monotonic milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reply {
    pub pull: Option<u32>,
    pub v: Option<u32>,
    /// When the Mac built the reply.
    pub t: Option<u64>,
    /// CPU busy percent, and when it was read.
    pub cpu: Option<f32>,
    pub tc: Option<u64>,
    /// GPU busy percent, and when macmon reported it.
    pub gpu: Option<f32>,
    pub tg: Option<u64>,
    /// Memory used in GB: the newest read, the low and high since the previous reply, and when
    /// the newest was read.
    pub mem: Option<f32>,
    pub mem_lo: Option<f32>,
    pub mem_hi: Option<f32>,
    pub tm: Option<u64>,
    pub mem_total: Option<f32>,
    /// False when macmon is stale.
    pub ok: Option<bool>,
}

/// Read one reply line, without its newline. None if the line is not a flat JSON object.
pub fn parse(line: &[u8]) -> Option<Reply> {
    let mut s = Scan { b: line, i: 0 };
    let mut r = Reply::default();
    s.ws();
    s.eat(b'{')?;
    s.ws();
    if s.eat(b'}').is_none() {
        loop {
            s.ws();
            let key = s.string()?;
            s.ws();
            s.eat(b':')?;
            s.ws();
            let value = s.value()?;
            r.set(key, value);
            s.ws();
            match s.next()? {
                b',' => continue,
                b'}' => break,
                _ => return None,
            }
        }
    }
    s.ws();
    (s.i == line.len()).then_some(r)
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Num(&'a str),
    Bool(bool),
    /// A string or null: valid JSON, but no key this firmware reads has that type.
    Other,
}

impl Reply {
    fn set(&mut self, key: &[u8], value: Value) {
        let Value::Num(n) = value else {
            if let (b"ok", Value::Bool(b)) = (key, value) {
                self.ok = Some(b);
            }
            return;
        };
        match key {
            b"pull" => self.pull = n.parse().ok(),
            b"v" => self.v = n.parse().ok(),
            b"t" => self.t = n.parse().ok(),
            b"cpu" => self.cpu = n.parse().ok(),
            b"tc" => self.tc = n.parse().ok(),
            b"gpu" => self.gpu = n.parse().ok(),
            b"tg" => self.tg = n.parse().ok(),
            b"mem" => self.mem = n.parse().ok(),
            b"mem_lo" => self.mem_lo = n.parse().ok(),
            b"mem_hi" => self.mem_hi = n.parse().ok(),
            b"tm" => self.tm = n.parse().ok(),
            b"mem_total" => self.mem_total = n.parse().ok(),
            _ => {}
        }
    }
}

struct Scan<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Scan<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        (self.peek()? == c).then(|| self.i += 1)
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    fn literal(&mut self, word: &[u8]) -> Option<()> {
        let end = self.i + word.len();
        (self.b.get(self.i..end)? == word).then(|| self.i = end)
    }

    /// A string, without its quotes. An escape is kept as is: no key we read has one.
    fn string(&mut self) -> Option<&'a [u8]> {
        self.eat(b'"')?;
        let start = self.i;
        loop {
            match self.next()? {
                b'"' => return Some(&self.b[start..self.i - 1]),
                b'\\' => {
                    self.next()?;
                }
                _ => {}
            }
        }
    }

    fn value(&mut self) -> Option<Value<'a>> {
        match self.peek()? {
            b'"' => self.string().map(|_| Value::Other),
            b't' => self.literal(b"true").map(|_| Value::Bool(true)),
            b'f' => self.literal(b"false").map(|_| Value::Bool(false)),
            b'n' => self.literal(b"null").map(|_| Value::Other),
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                while matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'-' | b'+' | b'e' | b'E')) {
                    self.i += 1;
                }
                core::str::from_utf8(&self.b[start..self.i]).ok().map(Value::Num)
            }
            // Nested objects and arrays are not part of the reply.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real line from tach-sender.py's build_reply.
    const FULL: &[u8] = br#"{"pull":7,"v":2,"t":1000,"hz":10.0,"cpu":12.3,"tc":999,"gpu":45.7,"tg":950,"mem":61.23,"mem_lo":60.9,"mem_hi":61.46,"tm":990,"mem_total":128.0,"ok":true}"#;

    #[test]
    fn reads_every_key_of_a_full_reply() {
        let r = parse(FULL).unwrap();
        assert_eq!((r.pull, r.v, r.t), (Some(7), Some(2), Some(1000)));
        assert_eq!((r.cpu, r.tc), (Some(12.3), Some(999)));
        assert_eq!((r.gpu, r.tg), (Some(45.7), Some(950)));
        assert_eq!((r.mem, r.mem_lo, r.mem_hi, r.tm), (Some(61.23), Some(60.9), Some(61.46), Some(990)));
        assert_eq!((r.mem_total, r.ok), (Some(128.0), Some(true)));
    }

    #[test]
    fn a_key_the_mac_left_out_is_none() {
        let r = parse(br#"{"pull":8,"v":2,"t":5,"mem_total":64.0,"ok":false}"#).unwrap();
        assert_eq!((r.cpu, r.gpu, r.mem, r.tc), (None, None, None, None));
        assert_eq!(r.ok, Some(false));
    }

    #[test]
    fn unknown_keys_and_strings_are_skipped() {
        let r = parse(br#"{ "note" : "a \"b\" c", "extra":null, "cpu" : 1.5 , "later":[] }"#);
        assert_eq!(r, None, "an array is not part of the reply");
        let r = parse(br#"{ "note" : "a \"b\" c", "extra":null, "cpu" : 1.5 }"#).unwrap();
        assert_eq!(r.cpu, Some(1.5));
    }

    #[test]
    fn a_value_of_the_wrong_type_is_missing() {
        let r = parse(br#"{"t":1.5,"pull":-1,"cpu":true,"ok":1}"#).unwrap();
        assert_eq!((r.t, r.pull, r.cpu, r.ok), (None, None, None, None));
    }

    #[test]
    fn rejects_lines_that_are_not_one_flat_object() {
        for line in [&b""[..], b"{", b"{\"cpu\":1", b"{\"cpu\":1}x", b"[1]", b"{\"a\":{}}", b"{cpu:1}"] {
            assert_eq!(parse(line), None, "{:?}", core::str::from_utf8(line));
        }
        assert_eq!(parse(b" {} "), Some(Reply::default()));
    }
}
