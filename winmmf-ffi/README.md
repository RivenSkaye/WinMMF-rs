# FFI wrapper for WinMMF

This is a wrapper for WinMMF designed to use it from other languages. The main idea is that the only extensively documented interface is the WinSDK one. There is an interface available in dotnet, but the documentation leaves a fair bit to be desired. It mentions use as IPC, but then only for sharing with child processes or not sharing at all. Other languages and runtimes don't seem to wrap and provide the API at all, or only through direct OS bindings. This crate would serve as an intermediary layer to provide a unified, well-defined, ergonomic API that can be made usable anywhere.
No guarantees are made about bindgen outputs. I would expect it to provide well-formed output and possibly even the correct checks for the use of non-zero integer types, but it'd be best to still manually do some checking and cleanup.

> [!NOTE]
> Please do not use this crate as a safe wrapper for use in C or C++. While it would probably work, you'll only hurt performance. WinMMF uses microseh (a thin C wrapper for Windows' very special exception model) so it should in most or all cases be faster to just use `__try` and `__except` and the MSVC toolchain. For most use cases, LLVM has you covered with their implementations of those error handling steps, behind `-fms-compatibility`. If you need Windows headers to build against, you might want to see where xwin gets them.

## Wrappers for languages

Are you missing a language? Feel free to PR it in!

A brief summary of the language for which bindgen exists and an effort is being made to keep it officially supported. The bindgen output can be enabled through custom config flags that optionally let you set the output filename, but with sane defaults. Flags can be provided in the following forms:

- `--cfg gen<LANG>` OR
- `--cfg gen<LANG>=outfile.name`

Where `<LANG>` is the value listed behind the language name. The generated file(s) can be found in `$OUT_DIR/../../generated/`, e.g. `./target/debug/build/generated/`

For ease of use, the table below includes the actual flag values.

| Language | Value | Flag  |                     Bindgen                     |
|----------|-------|-------|-------------------------------------------------|
| C#       | CS    | gencs | [csbindgen](https://crates.io/crates/csbindgen) |
