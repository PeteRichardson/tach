// Bake the BDF fonts in fonts/ into Rust statics (the glyph atlas, ADR-0001). Shared by
// device/build.rs and device/host-tests/build.rs through `include!`, so the firmware and its host
// tests read one atlas. The output is `$OUT_DIR/fonts.rs`, which src/text.rs includes.

use std::fmt::Write as _;
use std::path::Path;

/// (static name, BDF file in fonts/).
const FONTS: [(&str, &str); 2] = [("DIGITS", "tach-36.bdf"), ("LABELS", "tach-16.bdf")];

/// Write `$OUT_DIR/fonts.rs` from the BDF files in `fonts_dir`.
fn bake_fonts(fonts_dir: &Path) {
    let mut out = String::new();
    for (name, file) in FONTS {
        let path = fonts_dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let bdf = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out += &bake(name, file, &bdf);
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("fonts.rs");
    std::fs::write(dest, out).unwrap();
}

/// One font as a `pub static NAME: Font`. Panics with the file name on a BDF it cannot read: a
/// broken font must stop the build, not show up as a blank slot on the glass.
fn bake(name: &str, file: &str, bdf: &str) -> String {
    let mut out = format!("/// Baked from fonts/{file}.\npub static {name}: Font = Font {{ glyphs: &[\n");
    let mut lines = bdf.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("STARTCHAR") {
            continue;
        }
        let (mut code, mut adv, mut bbx) = (None, None, None);
        for line in lines.by_ref() {
            let mut f = line.split_whitespace();
            let key = f.next();
            let nums: Vec<i32> = f.map(|n| n.parse().expect(file)).collect();
            match key {
                Some("ENCODING") => code = Some(nums[0]),
                Some("DWIDTH") => adv = Some(nums[0]),
                Some("BBX") => bbx = Some((nums[0], nums[1], nums[2], nums[3])),
                Some("BITMAP") => break,
                _ => {}
            }
        }
        let (code, adv) = (code.expect(file), adv.expect(file));
        let (w, h, x, y) = bbx.expect(file);
        assert!((32..127).contains(&code), "{file}: glyph {code} is not printable ASCII");
        let mut bits = Vec::new();
        for line in lines.by_ref().take(h as usize) {
            for i in (0..line.len()).step_by(2) {
                bits.push(u8::from_str_radix(&line[i..i + 2], 16).expect(file));
            }
        }
        assert_eq!(bits.len(), h as usize * ((w as usize + 7) / 8), "{file}: glyph {code} bitmap");
        // BDF gives the bottom of the box above the baseline; the atlas keeps the top, y down.
        let top = -(y + h);
        let _ = writeln!(
            out,
            "    Glyph {{ ch: b'{}', adv: {adv}, x: {x}, top: {top}, w: {w}, h: {h}, bits: &{bits:?} }},",
            (code as u8 as char).escape_default()
        );
    }
    out + "] };\n"
}
