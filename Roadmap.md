# Roadmap for this repo

This crate, as it exists now, serves mainly for one active project I have at work. That said, I do intend to use it in more places, as well as to make it more useful for other people than it is now.  
There is a possibility that different language bindings will be crates of their own, in order to let people build and download only those bindings that they need.

## WinMMF

- [x] Working MMF implementation
- [x] Pointer size-agnostic for use on 32-bit and 64-bit platforms
- [ ] Partial views
- [ ] Resizable mapped views (really, mapping a new view on the same MMF)
- [ ] Dumping to disk
- [ ] Ease custom lock usage
- [ ] Document everything

## WinMMF-FFI

- [ ] Wrap MMF
- [ ] Allow opening several MMFs
- [ ] Set up easy use with bindgen for different languages
