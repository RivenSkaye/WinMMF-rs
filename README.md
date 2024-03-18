# Memory-Mapped Files

When working with Windows, there isn't a really ergonomic `shm` or `mmap` implementation that lets you share regions of memory across the application boundary easily. Instead the usual ways are calling back to localhost, (Named) Pipes, RPC, or Sockets (including the [broken `AF_UNIX` implementation](https://github.com/microsoft/WSL/issues/4240)). None of these are viable for low latency and/or large amounts of data being exchanged however, as they all use the networking stack and some of these don't even support communicating with multiple external processes. And if you don't need a message  and sometimes you just need to move more data, or move it faster on the local system, than what these methods allow. Enter [Named shared memory](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory), the roundabout way of mapping a named region of memory backed by the pagefile!

This crate exists to support this IPC mechanism in Rust. With the power of [microSEH](https://github.com/sonodima/microseh) to handle Structured Exception [Hissy fits](https://www.merriam-webster.com/dictionary/hissy%20fit) and some well thought out ways to make using this as safe as possible, it should be fine to use. I'm currently working on getting this production ready for some real-world use cases and slowly expanding it to support more architectures. It's very much still a WIP crate though, and as such is not yet available on crates.io at this time.

## Supported platforms

This crate only supports Windows. Nothing else uses this mechanic for shared memory to the best of my knowledge and it sounds painful to even test it. If you find it works in other places, let me know and I'll list it!

| Architecture |   OS    | Supported |                           Notes                           |
|--------------|---------|-----------|-----------------------------------------------------------|
| x86          | Windows | Yes       | Just Works:tm:                                            |
| x86_64/AMD64 | Windows | Yes       | Currently limited to the same ranges and sizes as 32-bit  |
| ARM          | Windows | Maybe?    | Have not tested this, nor do I have a system to test with |
| lolwut?      | ?????   | No        | What other architectures does Windows even run on?        |
| *            | *NIX    | lolnope   | No Kernel/OS support for these functions, try wine :joy:  |

## Goals

Honestly, I just want a working method of multiplexing a camera feed to a couple applications at the same time. And the biggest issues there are exclusive access and camera startup time and playback delay. But yeah, the goal is to make a stable MMF wrapper that handles the headache of the OS API surrounding it.

## License

This crate is brought to you under shared MIT or Apache 2.0 licensing, at your option. Contributions made will share this licensing unless explicitly stated otherwise. When stating other licensing on your work, please ensure it's compatible with the existing licenses. License texts can be found in the [MIT License](./LICENSE_MIT) and the [Apache 2.0 license](./LICENSE-APACHE) files.
