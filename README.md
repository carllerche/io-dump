# Io Dump

Rust library providing an I/O handle wrapper that passes through all reads and
writes while logging activity to a configurable destination.

[Documentation](https://docs.rs/io-dump)

## Usage

First, add this to your `Cargo.toml`:

```toml
[dependencies]
io-dump = "0.1"
```

Next, add this to your crate:

```rust
extern crate io_dump;

use io_dump::Dump;
```

# License

`io-dump` is primarily distributed under the terms of both the MIT license and
the Apache License (Version 2.0), with portions covered by various BSD-like
licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
