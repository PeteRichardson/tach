include!("bake_fonts.rs");

fn main() {
    println!("cargo::rustc-link-search=.",);
    println!("cargo::rustc-link-arg=-Tlink.x",);
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=bake_fonts.rs");
    bake_fonts(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts"));
}
