use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, UdpSocket};

const DEFAULT_PORT: u16 = 4000;

fn local_ip() -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("192.0.2.1:80")?;
    Ok(socket.local_addr()?.ip())
}

fn usage() -> ! {
    eprintln!("usage: share [-k] [-p port] file");
    std::process::exit(1);
}

fn main() -> std::io::Result<()> {
    let mut pargs = pico_args::Arguments::from_env();

    let keep_open = pargs.contains(["-k", "--keep-open"]);
    let port: u16 = pargs
        .opt_value_from_str(["-p", "--port"])
        .unwrap_or_else(|_| usage())
        .unwrap_or(DEFAULT_PORT);
    let file_path: String = pargs.free_from_str().unwrap_or_else(|_| usage());

    let body = fs::read(&file_path)?;
    let listener = TcpListener::bind(("0.0.0.0", port))?;

    let size = body.len() as f64;
    let (size, unit) = if size >= 1024.0 * 1024.0 {
        (size / 1024.0 / 1024.0, "MiB")
    } else {
        (size / 1024.0, "KiB")
    };

    println!("Sharing [{:.1}] {} @ {}:{}", size, unit, local_ip()?, port);

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
