use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, UdpSocket};

const PORT: u16 = 4000;

fn local_ip() -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("192.0.2.1:80")?;
    Ok(socket.local_addr()?.ip())
}

fn main() -> std::io::Result<()> {
    let mut args = env::args().skip(1);
    let file_path = args.next().unwrap_or_else(|| {
        eprintln!("usage: share [-k] file");
        std::process::exit(1);
    });
    let keep_open = args.any(|a| a == "-k" || a == "--keep-open");

    let body = fs::read(&file_path)?;
    let listener = TcpListener::bind(("0.0.0.0", PORT))?;

    println!("Sharing {:.1} KiB @ {}:{}", body.len() as f64 / 1024.0, local_ip()?, PORT);

    let mut buf = [0u8; 4096];

    for stream in listener
        .incoming()
        .take(if keep_open { usize::MAX } else { 1 })
    {
        let mut stream = stream?;
        println!("Connection from {}", stream.peer_addr()?);

        let n = stream.read(&mut buf)?;
        let request = String::from_utf8_lossy(&buf[..n]);

        request.lines()
            .filter(|l| l.get(..11).is_some_and(|h| h.eq_ignore_ascii_case("User-Agent:")))
            .for_each(|line| println!("{line}"));

        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        stream.write_all(header.as_bytes())?;
        stream.write_all(&body)?;
        stream.shutdown(Shutdown::Write)?;
    }

    Ok(())
}
