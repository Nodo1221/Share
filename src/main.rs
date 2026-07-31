use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};

const KEEP_OPEN: bool = false;
const FILE_PATH: &str = "src/index.html";

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:7878")?;
    let mut buf = [0u8; 4096];

    for stream in listener
        .incoming()
        .take(if KEEP_OPEN { usize::MAX } else { 1 })
    {
        let mut stream = stream?;
        println!("Connection from {}", stream.peer_addr()?);

        let n = stream.read(&mut buf)?;
        let request = String::from_utf8_lossy(&buf[..n]);

        for line in request.lines() {
            if line.get(..11).is_some_and(|h| h.eq_ignore_ascii_case("User-Agent:")) {
                println!("{line}");
            }
        }

        let body = fs::read(FILE_PATH)?;
        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());

        stream.write_all(header.as_bytes())?;
        stream.write_all(&body)?;
        stream.shutdown(Shutdown::Write)?;
    }

    Ok(())
}