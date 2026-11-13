fn main() {
    println!("cargo::rustc-link-search=.",);
    println!("cargo::rustc-link-arg=-Tlink.x",);
    println!("cargo:rerun-if-changed=memory.x");
}
