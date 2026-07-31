use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};

const KEEP_OPEN: bool = false;

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:7878")?;
    let mut buf = [0u8; 4096];

    for stream in listener
        .incoming()
        .take(if KEEP_OPEN { usize::MAX } else { 1 })
    {
        let mut stream = stream?;
        println!("Connection from {}", stream.peer_addr()?);

        let n = stream.read(&mut buf)?;
        std::io::stdout().write_all(&buf[..n])?;

        stream.write_all(b"Data\n")?;
        stream.shutdown(Shutdown::Write)?;
    }

    Ok(())
}
