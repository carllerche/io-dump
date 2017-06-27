//! Wraps an I/O handle, logging all activity in a readable format to a
//! configurable destination.

#![deny(warnings, missing_docs, missing_debug_implementations)]

#[macro_use]
extern crate log;

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
            dump: dump,
            now: Instant::now(),
        }
    }

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

            /*
            if i + 1 < line.len() {
                try!(write!(self.dump, " "));
            }
            */
        }

        write!(self.dump, "\n")
    }
}

impl<T: Read, U: Write> Read for Dump<T, U> {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let n = try!(self.upstream.read(dst));
        try!(self.write_packet(Direction::Read, &dst[0..n]));
        Ok(n)
    }
}

impl<T: Write, U: Write> Write for Dump<T, U> {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let n = try!(self.upstream.write(src));
        try!(self.write_packet(Direction::Write, &src[0..n]));

        trace!("{:?}", &src[0..n]);

        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        try!(self.upstream.flush());
        Ok(())
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
