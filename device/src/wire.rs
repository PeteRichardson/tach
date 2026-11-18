//! The reply line from tach-sender.py, read without an allocator (docs/device-v2.md, "Wire").
//!
//! The reply is one flat JSON object of numbers and booleans. This reader takes exactly that
//! shape: no nesting, and a string value is skipped, except `cores`. A key it does not know is skipped too, so the
//! Mac can add keys without a firmware change. A value that does not parse as its key's type is
//! treated as missing, the same as a key the Mac left out.

/// The protocol version this firmware reads. A reply with another `v` is not used.
pub const VERSION: u32 = 2;

/// The most cores a reply can carry. The Mac Studio this was built for has 36.
pub const MAX_CORES: usize = 64;

/// Busy percent of each core, 0 to 100, in the order the Mac sent them: the super cores first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cores {
    pct: [u8; MAX_CORES],
    n: u8,
}

impl Cores {
    /// One byte of hex for each core. None if the text is not whole bytes of hex, or has more
    /// than MAX_CORES of them. A byte past 100 reads as 100.
    fn from_hex(hex: &[u8]) -> Option<Cores> {
        if hex.len() % 2 != 0 || hex.len() > 2 * MAX_CORES {
            return None;
        }
        let digit = |c: u8| (c as char).to_digit(16);
        let mut pct = [0; MAX_CORES];
        for (p, pair) in pct.iter_mut().zip(hex.chunks(2)) {
            *p = (digit(pair[0])? * 16 + digit(pair[1])?).min(100) as u8;
        }
        Some(Cores { pct, n: (hex.len() / 2) as u8 })
    }

    pub fn percents(&self) -> &[u8] {
        &self.pct[..self.n as usize]
    }
}

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
    /// Busy percent of each core, read at `tc`, and how many of them are super cores.
    pub cores: Option<Cores>,
    pub nsuper: Option<u32>,
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
    Str(&'a [u8]),
    /// Null: valid JSON, but no key this firmware reads has that type.
    Other,
}

impl Reply {
    fn set(&mut self, key: &[u8], value: Value) {
        let Value::Num(n) = value else {
            match (key, value) {
                (b"ok", Value::Bool(b)) => self.ok = Some(b),
                (b"cores", Value::Str(hex)) => self.cores = Cores::from_hex(hex),
                _ => {}
            }
            return;
        };
        match key {
            b"pull" => self.pull = n.parse().ok(),
            b"v" => self.v = n.parse().ok(),
            b"t" => self.t = n.parse().ok(),
            b"cpu" => self.cpu = n.parse().ok(),
            b"tc" => self.tc = n.parse().ok(),
            b"nsuper" => self.nsuper = n.parse().ok(),
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
            b'"' => self.string().map(Value::Str),
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
    fn reads_per_core_percents_from_hex() {
        let r = parse(br#"{"cores":"00320a64ff","nsuper":2}"#).unwrap();
        let c = r.cores.unwrap();
        assert_eq!(c.percents(), &[0, 50, 10, 100, 100], "past 100 clamps to 100");
        assert_eq!(r.nsuper, Some(2));
    }

    #[test]
    fn bad_core_hex_is_missing() {
        for line in [&br#"{"cores":"0"}"#[..], br#"{"cores":"zz"}"#, br#"{"cores":7}"#] {
            assert_eq!(parse(line).unwrap().cores, None, "{:?}", core::str::from_utf8(line));
        }
        let long = [b'1'; 2 * MAX_CORES + 2];
        let mut line = b"{\"cores\":\"".to_vec();
        line.extend_from_slice(&long);
        line.extend_from_slice(b"\"}");
        assert_eq!(parse(&line).unwrap().cores, None, "more cores than the firmware keeps");
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
