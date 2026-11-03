# liminalkit-test-fonts

Noto Sans Regular 2.004 and Noto Sans Thai Regular 2.000, as a `FontSet` for liminalkit's tests and examples. `publish = false`: this crate exists so the font data never ends up in a published crate or an application build. Applications embed their own faces with `Face::from_bytes`. What the files are and where they came from is in [`fonts/README.md`](fonts/README.md).
