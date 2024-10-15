pub fn main() {
    if !(std::env::var_os("CARGO_CFG_WINDOWS").is_some() || std::env::var_os("DOCS_RS").is_some()) {
        println!(
            "cargo::warning=WinMMF-ffi: This crate only works for Windows targets. Please disable usage and references on other OSes.\nNothing is guaranteed to work when cross-compiling!"
        )
    }
    println!("cargo::rustc-cfg=windows_slim_errors")
}
