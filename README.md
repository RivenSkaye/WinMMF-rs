# WinMMF Workspace

This workspace aims to provide the main `winmmf` package as it exists on crates.io, but as additional crates built around it will be developed, they're organized in one place. As crates get added, this README will be updated.

## Crates

- [winmmf](./winmmf), the actual Rust wrapper for Memory Mapped Files
- [winmmf-ffi](./winmmf-ffi/), `pub extern "C"` API for using WinMMF from other languages

### versioning

The crates here live in pretty standard semver. The only thing that stands out is that every API-breaking change in `WinMMF` will cause all crates in the workspace to be bumped to the next indicative version. As the workspace is currently on 0.x.y, this means it bumps to 0.z.0.

## MSRV

The MSRV is manually checked before releasing as of workspace version 0.2.1. This is done with the help of [`cargo-msrv`](https://gribnau.dev/cargo-msrv/index.html). I might integrate it in CI at some point, but there are currently no plans to do so.

The current listed MSRV is: **1.75**.

## Supported platforms

This crate only supports Windows. Nothing else uses this mechanic for shared memory to the best of my knowledge and it sounds painful to even test it. If you find it works in other places, let me know and I'll list it!  
If there's something you'd like to see changed, open an issue explaining why and it'll get looked at.

| Architecture | Officially Supported | Works |                           Notes                                                                           |
|--------------|----------------------|-------|-----------------------------------------------------------------------------------------------------------|
| x86          | Yes                  | Yes   | Just Works™.                                                                                              |
| x86_64/AMD64 | Yes                  | Yes   | Just Works™.                                                                                              |
| ARM          | No hardware          | Maybe | Have not tested this, nor do I have a system to test with. Contributors with Windows ARM machines wanted. |
| Other        | No                   | Maybe | I am not able to help support things I can't access. Community support would be welcome and listed here.  |

> [!TIP]
> These crates _might_ be usuable on cross toolchains using [xwin](https://github.com/Jake-Shadle/xwin) and/or leveraging [cargo-xwin](https://github.com/rust-cross/cargo-xwin). No warranties or guarantees are provided, but there is a willingness to provide support for this once this crate reaches a point of maturity and stability that allows for allocating resources towards this end.

## License

All crates in this workspace are brought to you under the MPL-2.0. Contributions made will share this licensing unless explicitly stated otherwise. When stating other licensing on your work, please ensure it's compatible with the existing license. License texts can be found in the [LICENSE file](./LICENSE).
