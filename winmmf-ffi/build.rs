pub fn main() {
    if !(std::env::var_os("CARGO_CFG_WINDOWS").is_some() || std::env::var_os("DOCS_RS").is_some()) {
        println!(
            "cargo:warning=WinMMF-ffi: This crate only works for Windows targets. Please disable usage and references on other OSes.\nNothing is guaranteed to work when cross-compiling!"
        )
    }
    println!("cargo:rustc-cfg=windows_slim_errors");
    let outpath = std::env::var("OUT_DIR").unwrap() + "/../../generated/";
    if let Some(cfg_filename) = std::env::var_os("CARGO_CFG_GENCS") {
        let csfile = match cfg_filename.len() {
            0 => "winmmf.g.cs",
            _ => cfg_filename.to_str().unwrap_or("winmmf.g.cs"),
        };
        csbindgen::Builder::default()
            .input_extern_file("src/lib.rs")
            .csharp_dll_name("winmmf")
            .csharp_class_name("MemoryMappedFilesRS")
            .csharp_namespace("Azor.Rust")
            .csharp_class_accessibility("public")
            .generate_csharp_file(outpath + csfile)
            .unwrap();
    }
}
