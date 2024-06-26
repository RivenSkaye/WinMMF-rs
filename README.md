# Memory-Mapped Files

When working with Windows, there isn't a really ergonomic `shm` or `mmap` implementation that lets you share regions of memory across the application boundary easily. Instead the usual ways are calling back to localhost, (Named) Pipes, RPC, or Sockets (including the [broken `AF_UNIX` implementation](https://github.com/microsoft/WSL/issues/4240)). None of these are viable for low latency and/or large amounts of data being exchanged however, as they all use the networking stack and some of these don't even support communicating with multiple external processes. And if you don't need a message and sometimes you just need to move more data, or move it faster on the local system, than what these methods allow. Enter [Named shared memory](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory), the roundabout way of mapping a named region of memory backed by the pagefile!

This crate exists to support this IPC mechanism in Rust. With the power of [microSEH](https://github.com/sonodima/microseh) to handle Structured Exception [Hissy fits](https://www.merriam-webster.com/dictionary/hissy%20fit) and some well thought out ways to make using this as safe as possible, it should be fine to use. I'm currently working on getting this production ready for some real-world use cases and slowly expanding it to support more use cases. It's very much still a WIP crate though.

## WIP CRATE, DO NOT USE

This is not a joke or a drill. I hate having to add this, and I made sure the current version on crates.io does not yet compile to prevent its use.
I need the name reserved, that is all.

## Supported platforms

This crate only supports Windows. Nothing else uses this mechanic for shared memory to the best of my knowledge and it sounds painful to even test it. If you find it works in other places, let me know and I'll list it!  
If there's something you'd like to see changed, open an issue explaining why and it'll get looked at. By the way, mapping more than a few gigs already crashes AMD64 builds, so I really doubt there's any reason to use 64-bit integers even there.

| Architecture |   OS    | Supported | Works |                           Notes                                                                               |
|--------------|---------|-----------|-------|---------------------------------------------------------------------------------------------------------------|
| x86          | Windows | Yes       | Yes   | Just Works:tm:.                                                                                               |
| x86_64/AMD64 | Windows | Yes       | Yes   | Currently limited to 32-bit sizes, `u32::MAX` bytes should actually be enough for most use cases.             |
| ARM          | Windows | No        | Maybe | Have not tested this, nor do I have a system to test with. If someone can test, I'd gladly mark it supported. |
| Other        | Windows | No        | Maybe | I am not willing to help support things I can't access. If someone else wants to maintain support, feel free. |
| *            | *NIX    | lolnope   | No    | This entire crate is for wrapping the _Windows_ API for sharing memory. Linux already has a good `shmem`.     |

## Goals

Being able to support sharing memory accross the application boundary. This includes providing ergonomic interfaces for screeching at people before things break (lacking permissions for example) and making it possible for easily accessing the data one needs to pass into child processes to share mem their way.
For now, this means _one large block_ of memory. Resizable mappings and whatnot are a planned feature, and a roadmap will be added at a later date.

## License

This crate is brought to you under the MPL-2.0. Contributions made will share this licensing unless explicitly stated otherwise. When stating other licensing on your work, please ensure it's compatible with the existing license. License texts can be found in the [LICENSE file](./LICENSE).
