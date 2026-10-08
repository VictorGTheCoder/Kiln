//! Test client for the dashboard's Server-Sent Events stream, over raw
//! HTTP/1.1 so the tests see exactly what the server sends.
#![allow(dead_code)]
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    time::Duration,
};

/// One Server-Sent Events frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub id: Option<String>,
    pub event: String,
    pub data: String,
}
impl Frame {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or_else(|e| panic!("{e}: {}", self.data))
    }
    pub fn is(&self, ticket: Option<&str>, stage: &str, status: &str) -> bool {
        if self.event != "journal" {
            return false;
        }
        let event = self.json();
        event["ticket"].as_str() == ticket && event["stage"] == stage && event["status"] == status
    }
}
/// The body of a chunked HTTP/1.1 response; ends at its last chunk.
struct Dechunk {
    inner: BufReader<TcpStream>,
    left: usize,
    done: bool,
}
impl Read for Dechunk {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            self.inner.read_line(&mut size)?;
            self.left = usize::from_str_radix(size.trim(), 16)
                .unwrap_or_else(|_| panic!("chunk size line {size:?}"));
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let wanted = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..wanted])?;
        self.left -= n;
        if self.left == 0 {
            let mut crlf = String::new();
            self.inner.read_line(&mut crlf)?;
        }
        Ok(n)
    }
}
/// A live connection to an event stream.
pub struct Stream {
    pub status: u16,
    pub head: String,
    reader: BufReader<Dechunk>,
}
impl Stream {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (field, value) = line.split_once(':')?;
            field.eq_ignore_ascii_case(name).then_some(value.trim())
        })
    }
    /// The next frame carrying data; `None` once the server closes the stream.
    pub fn next(&mut self) -> Option<Frame> {
        let (mut id, mut event, mut data) = (None, "message".to_owned(), Vec::new());
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line).unwrap() == 0 {
                return None;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !data.is_empty() {
                    return Some(Frame {
                        id,
                        event,
                        data: data.join("\n"),
                    });
                }
                continue;
            }
            match line.split_once(':') {
                Some(("", _)) => {} // comment, e.g. a keep-alive
                Some(("id", v)) => id = Some(v.trim_start().to_owned()),
                Some(("event", v)) => event = v.trim_start().to_owned(),
                Some(("data", v)) => data.push(v.strip_prefix(' ').unwrap_or(v).to_owned()),
                _ => {}
            }
        }
    }
    /// Frames up to and including the first matching one.
    pub fn until(&mut self, mut done: impl FnMut(&Frame) -> bool) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Some(frame) = self.next() {
            let stop = done(&frame);
            frames.push(frame);
            if stop {
                return frames;
            }
        }
        panic!("stream closed before the expected frame: {frames:#?}");
    }
}
/// Open `GET /api/dashboard/runs/<id>/events` on the dashboard at `address`.
pub fn connect(address: &str, id: &str, last_event_id: Option<&str>, host: &str) -> Stream {
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let resume = last_event_id
        .map(|id| format!("Last-Event-ID: {id}\r\n"))
        .unwrap_or_default();
    write!(
        &stream,
        "GET /api/dashboard/runs/{id}/events HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n{resume}\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        head.push_str(&line);
    }
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    Stream {
        status,
        head,
        reader: BufReader::new(Dechunk {
            inner: reader,
            left: 0,
            done: status != 200 || !chunked,
        }),
    }
}
