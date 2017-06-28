//! Wraps an I/O handle, logging all activity in a readable format to a
//! configurable destination.
//!
//! # Overview
//!
//! [`Dump`] decorates an I/O handle that implements [`Read`] and/or [`Write`]. [`Dump`]
//! then passes calls to [`read`] and [`write`] through to the inner I/O handle
//! while also logging the packets to a configurable destination in readable
//! format.
//!
//! This can be useful for debugging protocols that are encrypted and generating
//! reproducible test cases.
//!
//! In the encrypted case, `Dump` can be be added after the decryption step but
//! before the application logic step. For example, with SSL, `Dump` would wrap
//! [`TlsStream`] and the dump can be written to STDOUT.
//!
//! For reproducing test cases, a `TcpStream` could be wrapped and the log
//! written to a file. Then [fixture-io] can load the file and replay the data
//! exchange. These replayable scenarios can be used as part of unit tests to
//! help prevent regressions.
//!
//! # Usage
//!
//! Add the following to your `Cargo.toml`
//!
//! ```toml
//! [dependencies]
//! io-dump = { git = "github.com/carllerche/io-dump" }
//! ```
//!
//! Then use it in your project. Wrap a stream with `Dump` then use the wrapped
//! stream as you would have otherwise.
//!
//! For example:
//!
//! ```no_run
//! extern crate io_dump;
//!
//! # pub fn main() {
//! use io_dump::Dump;
//! use std::io::prelude::*;
//! use std::net::TcpStream;
//!
//! let stream = TcpStream::connect("127.0.0.1:34254").unwrap();
//! let mut stream = Dump::to_stdout(stream);
//!
//! let _ = stream.write(&[1]);
//! let _ = stream.read(&mut [0; 128]); // ignore here too
//! # }
//! ```
//!
//! **Note** that writing the log output is done using blocking I/O. So, writing
//! to a file could block the current thread if the disk is not ready. This
//! could cause delays in non-blocking systems such as Tokio. As such, care
//! should be taken when using `io-dump` in production systems.
//!
//! # File format
//!
//! `io-dump` uses a custom file output. Each packet begins with a header line
//! consisting of:
//!
//! * The packet direction (either read or write)
//! * The timestamp represented as milliseconds elapsed since the dump start.
//! * The number of bytes in the packet payload.
//!
//! The packet direction is represented as `<-` for read and `->` for write.
//!
//! After the header line, the packet payload is written in two columns. The
//! first column is hex encoded and the second column attempts to provide an
//! ASCII representation.
//!
//! New lines between packets and comments (`//` prefixed) are ignored.
//!
//! Example:
//!
//! ```not_rust,no_wrap
//! // Client connection preface
//! <-  0.001s  24 bytes
//! 50 52 49 20 2A 20 48 54 54 50 2F 32 2E 30     P R I   *   H T T P / 2 . 0
//! 0D 0A 0D 0A 53 4D 0D 0A 0D 0A                 \r\n\r\n S M\r\n\r\n
//!
//! // Settings
//! <-  0.001s  39 bytes
//! 00 00 1E 04 00 00 00 00 00 00 01 00 00 10     \0\0\?\?\0\0\0\0\0\0\?\0\0\?
//! 00 00 02 00 00 00 01 00 03 00 00 00 64 00     \0\0\?\0\0\0\?\0\?\0\0\0 d\0
//! 04 00 00 FF FF 00 05 00 00 40 00              \?\0\0\?\?\0\?\0\0 @\0
//!
//! // Request headers
//! <-  0.002s  30 bytes
//! 00 00 15 01 05 00 00 00 01 82 87 01 10 68     \0\0\?\?\?\0\0\0\?\?\?\?\? h
//! 74 74 70 32 2E 61 6B 61 6D 61 69 2E 63 6F      t t p 2 . a k a m a i . c o
//! 6D 85                                          m\?
//!
//! // Settings frame
//! ->  0.013s  9 bytes
//! 00 00 1E 04 00 00 00 00 00                    \0\0\?\?\0\0\0\0\0
//! ```
//!
//! [`Dump`]: struct.Dump.html
//! [`Read`]: https://doc.rust-lang.org/std/io/trait.Read.html
//! [`Write`]: https://doc.rust-lang.org/std/io/trait.Write.html
//! [`read`]: https://doc.rust-lang.org/std/io/trait.Read.html#tymethod.read
//! [`write`]: https://doc.rust-lang.org/std/io/trait.Write.html#tymethod.write
//! [fixture-io]: github.com/carllerche/fixture-io
//! [`TlsStream`]: https://docs.rs/tokio-tls/0.1/tokio_tls/struct.TlsStream.html

#![deny(warnings, missing_docs, missing_debug_implementations)]

#[cfg(feature = "tokio")]
extern crate futures;

#[cfg(feature = "tokio")]
extern crate tokio_io;

use std::cmp;
use std::fs::File;
use std::io::{self, Read, Write, BufRead, BufReader, Lines};
use std::path::Path;
use std::time::{Instant, Duration};

/// Wraps an I/O handle, logging all activity in a readable format to a
/// configurable destination.
///
/// See [library level documentation](index.html) for more details.
#[derive(Debug)]
pub struct Dump<T, U> {
    upstream: T,
    inner: Option<Inner<U>>,
}

#[derive(Debug)]
struct Inner<U> {
    dump: U,
    now: Instant,
}

/// Read the contents of a dump
#[derive(Debug)]
pub struct DumpRead<T> {
    lines: Lines<BufReader<T>>,
}

/// Unit of data either read or written.
#[derive(Debug)]
pub struct Packet {
    head: Head,
    data: Vec<u8>,
}

/// Direction in which a packet was transfered.
///
/// A packet is either read from an io source or it is written to an I/O handle.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Direction {
    /// Data is read from an I/O handle.
    Read,

    /// Data is written to an I/O handle.
    Write,
}

#[derive(Debug)]
struct Head {
    direction: Direction,
    elapsed: Duration,
}

impl Packet {
    /// The packet direction. Either `Read` or `Write`.
    pub fn direction(&self) -> Direction {
        self.head.direction
    }

    /// The elapsed duration from the dump creation time to the time the
    /// instance of the packet's occurance.
    pub fn elapsed(&self) -> Duration {
        self.head.elapsed
    }

    /// The data being transmitted.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/*
 *
 * ===== impl Dump =====
 *
 */

const LINE: usize = 25;

impl<T> Dump<T, File> {
    /// Dump `upstream`'s activity to a file located at `path`.
    ///
    /// If a file exists at `path`, it will be overwritten.
    pub fn to_file<P: AsRef<Path>>(upstream: T, path: P) -> io::Result<Self> {
        File::create(path)
            .map(move |dump| Dump::new(upstream, dump))
    }
}

impl<T> Dump<T, io::Stdout> {
    /// Dump `upstream`'s activity to STDOUT.
    pub fn to_stdout(upstream: T) -> Self {
        Dump::new(upstream, io::stdout())
    }
}

impl<T, U: Write> Dump<T, U> {
    /// Create a new `Dump` wrapping `upstream` logging activity to `dump`.
    pub fn new(upstream: T, dump: U) -> Dump<T, U> {
        Dump {
            upstream: upstream,
            inner: Some(Inner {
                dump: dump,
                now: Instant::now(),
            }),
        }
    }

    /// Create a new `Dump` that passes packets through without logging.
    pub fn noop(upstream: T) -> Dump<T, U> {
        Dump {
            upstream: upstream,
            inner: None,
        }
    }
}

impl<T: Read, U: Write> Read for Dump<T, U> {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let n = try!(self.upstream.read(dst));

        if let Some(inner) = self.inner.as_mut() {
            try!(inner.write_packet(Direction::Read, &dst[0..n]));
        }

        Ok(n)
    }
}

impl<T: Write, U: Write> Write for Dump<T, U> {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let n = try!(self.upstream.write(src));

        if let Some(inner) = self.inner.as_mut() {
            try!(inner.write_packet(Direction::Write, &src[0..n]));
        }

        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        try!(self.upstream.flush());
        Ok(())
    }
}

// ===== impl Inner =====

impl<U: Write> Inner<U> {
    fn write_packet(&mut self, dir: Direction, data: &[u8]) -> io::Result<()> {
        if dir == Direction::Write {
            try!(write!(self.dump, "<-  "));
        } else {
            try!(write!(self.dump, "->  "));
        }

        // Write elapsed time
        let elapsed = millis((Instant::now() - self.now)) as f64 / 1000.0;
        try!(write!(self.dump, "{:.*}s  {} bytes", 3, elapsed, data.len()));

        // Write newline
        try!(write!(self.dump, "\n"));

        let mut pos = 0;

        while pos < data.len() {
            let end = cmp::min(pos + LINE, data.len());
            try!(self.write_data_line(&data[pos..end]));
            pos = end;
        }

        try!(write!(self.dump, "\n"));

        Ok(())
    }

    fn write_data_line(&mut self, line: &[u8]) -> io::Result<()> {
        // First write binary
        for i in 0..LINE {
            if i >= line.len() {
                try!(write!(self.dump, "   "));
            } else {
                try!(write!(self.dump, "{:02X} ", line[i]));
            }
        }

        // Write some spacing for the ascii
        try!(write!(self.dump, "    "));

        for &byte in line.iter() {
            match byte {
                 0 => try!(write!(self.dump, "\\0")),
                 9 => try!(write!(self.dump, "\\t")),
                10 => try!(write!(self.dump, "\\n")),
                13 => try!(write!(self.dump, "\\r")),
                32...126 => {
                    try!(self.dump.write(&[b' ', byte]));
                }
                _ => try!(write!(self.dump, "\\?")),
            }
        }

        write!(self.dump, "\n")
    }
}

/*
 *
 * ===== impl DumpRead =====
 *
 */

impl DumpRead<File> {
    /// Open a dump file at the specified location.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let dump = try!(File::open(path));
        Ok(DumpRead::new(dump))
    }
}

impl<T: Read> DumpRead<T> {
    /// Reads dump packets from the specified source.
    pub fn new(io: T) -> DumpRead<T> {
        DumpRead { lines: BufReader::new(io).lines() }
    }

    fn read_packet(&mut self) -> io::Result<Option<Packet>> {
        loop {
            let head = match self.lines.next() {
                Some(Ok(line)) => line,
                Some(Err(e)) => return Err(e),
                None => return Ok(None),
            };

            let head: Vec<String> = head
                .split(|v| v == ' ')
                .filter(|v| !v.is_empty())
                .map(|v| v.into())
                .collect();

            if head.len() == 0 || head[0] == "//" {
                continue;
            }

            assert_eq!(4, head.len());

            let dir = match &head[0][..] {
                "<-" => Direction::Write,
                "->" => Direction::Read,
                _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid direction format")),
            };

            let elapsed: f64 = {
                let s = &head[1];
                s[..s.len()-1].parse().unwrap()
            };

            // Do nothing w/ bytes for now

            // ready body
            let mut data = vec![];

            loop {
                let line = match self.lines.next() {
                    Some(Ok(line)) => line,
                    Some(Err(e)) => return Err(e),
                    None => "".into(),
                };

                if line.is_empty() {
                    return Ok(Some(Packet {
                        head: Head {
                            direction: dir,
                            elapsed: Duration::from_millis((elapsed * 1000.0) as u64),
                        },
                        data: data,
                    }));
                }

                let mut pos = 0;

                loop {
                    let c = &line[pos..pos+2];

                    if c == "  " {
                        break;
                    }

                    let byte = match u8::from_str_radix(c, 16) {
                        Ok(byte) => byte,
                        Err(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "could not parse byte")),
                    };

                    data.push(byte);

                    pos += 3;
                }
            }
        }
    }
}

impl<T: Read> Iterator for DumpRead<T> {
    type Item = Packet;

    fn next(&mut self) -> Option<Packet> {
        self.read_packet().unwrap()
    }
}

const NANOS_PER_MILLI: u32 = 1_000_000;
const MILLIS_PER_SEC: u64 = 1_000;

/// Convert a `Duration` to milliseconds, rounding up and saturating at
/// `u64::MAX`.
///
/// The saturating is fine because `u64::MAX` milliseconds are still many
/// million years.
fn millis(duration: Duration) -> u64 {
    // Round up.
    let millis = (duration.subsec_nanos() + NANOS_PER_MILLI - 1) / NANOS_PER_MILLI;
    duration.as_secs().saturating_mul(MILLIS_PER_SEC).saturating_add(millis as u64)
}

#[cfg(feature = "tokio")]
mod tokio {
    use super::Dump;

    use futures::Poll;
    use tokio_io::{AsyncRead, AsyncWrite};

    use std::io::{self, Write};

    impl<T: AsyncRead, U: Write> AsyncRead for Dump<T, U> {}

    impl<T: AsyncWrite, U: Write> AsyncWrite for Dump<T, U> {
        fn shutdown(&mut self) -> Poll<(), io::Error> {
            self.upstream.shutdown()
        }
    }
}
