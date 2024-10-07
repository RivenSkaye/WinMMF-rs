# Memory-Mapped Files

When working with Windows, there isn't a really ergonomic `shm` or `mmap` implementation that lets you share regions of memory across the application boundary easily. Instead the usual ways are calling back to localhost, (Named) Pipes, RPC, or Sockets (including the [broken `AF_UNIX` implementation](https://github.com/microsoft/WSL/issues/4240)). None of these are viable for low latency and/or large amounts of data being exchanged however, as they all use the networking stack and some of these don't even support communicating with multiple external processes. And if you don't need a message and sometimes you just need to move more data, or move it faster on the local system, than what these methods allow. Enter [Named shared memory](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory), the roundabout way of mapping a named region of memory backed by the pagefile!

This crate exists to support this IPC mechanism in Rust. With the power of [microSEH](https://github.com/sonodima/microseh) to handle Structured Exception [Hissy fits](https://www.merriam-webster.com/dictionary/hissy%20fit) and some well thought out ways to make using this as safe as possible, it should be fine to use. I'm currently working on finding out how well this will run in a production environment, once some other challenges are solved causing issues in the environment it should run in.

This crate is currently in a state of baby steps. I've done some basic testing and it mostly works, but there's no full test suite yet. I'm also having some trouble figuring out what the best approach would be for this, as it'd require either multithreading and opening several handles to the same MMF, or permission elevation.  
Contributors are always welcome. Even more so if they can help cover what's lacking here, such as implementing a full test suite and implementing more architectures.

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
> This lib crate _might_ be usuable on cross toolchains using [xwin](https://github.com/Jake-Shadle/xwin) and/or leveraging [cargo-xwin](https://github.com/rust-cross/cargo-xwin). No warranties or guarantees are provided, but there is a willingness to provide support for this once this crate reaches a point of maturity and stability that allows for allocating resources towards this end.

## Goals

Being able to support sharing memory accross the application boundary. This includes providing ergonomic interfaces for screeching at people before things break (lacking permissions for example) and making it possible for easily accessing the data one needs to pass into child processes to share mem their way.
For now, this means _one large block_ of memory. Resizable mappings and whatnot are a planned feature, and a roadmap will be added at a later date.

## License

This crate is brought to you under the MPL-2.0. Contributions made will share this licensing unless explicitly stated otherwise. When stating other licensing on your work, please ensure it's compatible with the existing license. License texts can be found in the [LICENSE file](./LICENSE).
