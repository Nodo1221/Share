use std::io::{IsTerminal, Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, UdpSocket};

const DEFAULT_PORT: u16 = 4000;

fn local_ip() -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("192.0.2.1:80")?;
    Ok(socket.local_addr()?.ip())
}

fn usage() -> ! {
    eprintln!("usage: share [-k] [-p port] [file | -]");
    std::process::exit(1);
}

fn main() -> std::io::Result<()> {
    let mut pargs = pico_args::Arguments::from_env();

    let keep_open = pargs.contains(["-k", "--keep-open"]);
    let port: u16 = pargs
        .opt_value_from_str(["-p", "--port"])
        .unwrap_or_else(|_| usage())
        .unwrap_or(DEFAULT_PORT);

    let file_path: Option<String> = pargs.opt_free_from_str().unwrap_or_else(|_| usage());

    let body = match file_path.as_deref().unwrap_or("-") {
        "-" if std::io::stdin().is_terminal() => usage(),
        "-" => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            buf
        }
        path => std::fs::read(path)?,
    };

    let (size, unit) = match body.len() as f64 {
        s @ ..1_048_576.0 => (s / 1024.0, "KiB"),
        s => (s / 1024.0 / 1024.0, "MiB"),
    };

    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let mut buf = [0u8; 4096];

    println!("Sharing [{size:.1}{unit}] @ http://{}:{port}", local_ip()?);

    for stream in listener
        .incoming()
        .take(if keep_open { usize::MAX } else { 1 })
    {
        let mut stream = stream?;
        println!("\x1b[34m{}\x1b[0m", stream.peer_addr()?);

        let n = stream.read(&mut buf)?;
        let request = String::from_utf8_lossy(&buf[..n]);

        request
            .lines()
            .filter(|h| h.to_lowercase().starts_with("user-agent:"))
            .for_each(|line| println!("\t{line}"));

        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        stream.write_all(header.as_bytes())?;
        stream.write_all(&body)?;
        stream.shutdown(Shutdown::Write)?;
    }

    Ok(())
}
