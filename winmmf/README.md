# Memory-Mapped Files

When working with Windows, there isn't a really ergonomic `shm` or `mmap` implementation that lets you share regions of memory across the application boundary easily. Instead the usual ways are calling back to localhost, (Named) Pipes, RPC, or Sockets (including the [broken `AF_UNIX` implementation](https://github.com/microsoft/WSL/issues/4240)). None of these are viable for low latency and/or large amounts of data being exchanged however, as they all use the networking stack and some of these don't even support communicating with multiple external processes. And if you don't need a message and sometimes you just need to move more data, or move it faster on the local system, than what these methods allow. Enter [Named shared memory](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory), the roundabout way of mapping a named region of memory backed by the pagefile!

This crate exists to support this IPC mechanism in Rust. With the power of [microSEH](https://github.com/sonodima/microseh) to handle Structured Exception [Hissy fits](https://www.merriam-webster.com/dictionary/hissy%20fit) and some well thought out ways to make using this as safe as possible, it should be fine to use. I'm currently working on finding out how well this will run in a production environment, once some other challenges are solved causing issues in the environment it should run in.

This crate is currently in a state of baby steps. I've done some basic testing and it mostly works, but there's no full test suite yet. I'm also having some trouble figuring out what the best approach would be for this, as it'd require either multithreading and opening several handles to the same MMF, or permission elevation.  
Contributors are always welcome. Even more so if they can help cover what's lacking here, such as implementing a full test suite and implementing more architectures.

> [!TIP]
> The crate does not warrant expansive usage examples or scenario sketches of any kind. Most use cases will only ever use a very narrow API (by design) and those are all covered [in the unit tests](https://github.com/RivenSkaye/WinMMF-rs/tree/master/src/unit_tests) with the exception of `read_to_buff`. The difference with `read` being that the second argument should be a `&mut Vec<u8>`. You don't _have_ to worry about the Vec's capacity either, `read_to_buff` will just reserve more space if required!

## Goals

Being able to support sharing memory accross the application boundary. This includes providing ergonomic interfaces for screeching at people before things break (lacking permissions for example) and making it possible for easily accessing the data one needs to pass into child processes to share mem their way.
For now, this means _one large block_ of memory.
