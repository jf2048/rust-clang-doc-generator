# rust-clang-doc-generator

An experiment to copy docs from C sources into Rust sources. Scans Rust sources
for items annotated with `#[doc(alias = "my_c_func")]`, then scans C sources
for matching symbols. If the symbol is found, write a Rust doc comment with the
contents of the C doc comment.

Use `cargo run -- --help` for more information on how to use this.
