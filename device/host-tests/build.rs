include!("../bake_fonts.rs");

fn main() {
    println!("cargo:rerun-if-changed=../bake_fonts.rs");
    bake_fonts(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts"));
}
