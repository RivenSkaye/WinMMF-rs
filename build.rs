#[cfg(target_os = "windows")]
pub fn main() {
    println!("cargo::rustc-cfg=windows_slim_errors")
}

#[cfg(not(target_os = "windows"))]
pub fn main() {
    compile_error!("This crate is literally made for the Windows API. Please learn how conditional compilation works.")
}
