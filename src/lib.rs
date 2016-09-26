extern crate futures;
extern crate tokio_core;

#[macro_use]
extern crate log;

use std::cmp;
use std::fs::File;
use std::io::{self, Read, Write, BufRead, BufReader, Lines};
use std::path::Path;
use std::time::{Instant, Duration};

use futures::{Async};
use tokio_core::io::Io;

/// Copies all data read from and written to the upstream I/O to a file.
pub struct IoDump<T> {
    upstream: T,
    dump: File,
    now: Instant,
}

pub struct Dump {
    lines: Lines<BufReader<File>>,
}

pub struct Block {
    head: Head,
    data: Vec<u8>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Direction {
    In,
    Out,
}

struct Head {
    direction: Direction,
    elapsed: Duration,
}

impl Block {
    pub fn direction(&self) -> Direction {
        self.head.direction
    }

    pub fn elapsed(&self) -> Duration {
        self.head.elapsed
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/*
 *
 * ===== impl IoDump =====
 *
 */

const LINE: usize = 25;

impl<T> IoDump<T> {
    pub fn new<P: AsRef<Path>>(upstream: T, path: P) -> io::Result<IoDump<T>> {
        Ok(IoDump {
            upstream: upstream,
            dump: try!(File::create(path)),
            now: Instant::now(),
        })
    }

    fn write_block(&mut self, dir: Direction, data: &[u8]) -> io::Result<()> {
        if dir == Direction::In {
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

impl<T: Read> Read for IoDump<T> {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let n = try!(self.upstream.read(dst));
        try!(self.write_block(Direction::Out, &dst[0..n]));
        Ok(n)
    }
}

impl<T: Write> Write for IoDump<T> {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let n = try!(self.upstream.write(src));
        try!(self.write_block(Direction::In, &src[0..n]));

        trace!("{:?}", &src[0..n]);

        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        try!(self.upstream.flush());
        Ok(())
    }
}

impl<T: Io> Io for IoDump<T> {
    fn poll_read(&mut self) -> Async<()> {
        self.upstream.poll_read()
    }

    fn poll_write(&mut self) -> Async<()> {
        self.upstream.poll_write()
    }
}

/*
 *
 * ===== impl Dump =====
 *
 */

impl Dump {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Dump> {
        let dump = try!(File::open(path));
        let dump = BufReader::new(dump);
        Ok(Dump { lines: dump.lines() })
    }

    fn read_block(&mut self) -> io::Result<Block> {
        let head = match self.lines.next() {
            Some(Ok(line)) => line,
            Some(Err(e)) => return Err(e),
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF")),
        };

        let head: Vec<String> = head
            .split(|v| v == ' ')
            .filter(|v| !v.is_empty())
            .map(|v| v.into())
            .collect();

        assert_eq!(4, head.len());

        let dir = match &head[0][..] {
            "<-" => Direction::In,
            "->" => Direction::Out,
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
                None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF")),
            };

            if line.is_empty() {
                return Ok(Block {
                    head: Head {
                        direction: dir,
                        elapsed: Duration::from_millis((elapsed * 1000.0) as u64),
                    },
                    data: data,
                });
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

impl Iterator for Dump {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        self.read_block().ok()
    }
}

const NANOS_PER_MILLI: u32 = 1_000_000;
const MILLIS_PER_SEC: u64 = 1_000;

/// Convert a `Duration` to milliseconds, rounding up and saturating at
/// `u64::MAX`.
///
/// The saturating is fine because `u64::MAX` milliseconds are still many
/// million years.
pub fn millis(duration: Duration) -> u64 {
    // Round up.
    let millis = (duration.subsec_nanos() + NANOS_PER_MILLI - 1) / NANOS_PER_MILLI;
    duration.as_secs().saturating_mul(MILLIS_PER_SEC).saturating_add(millis as u64)
}
