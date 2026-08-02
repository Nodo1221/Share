use std::io::{IsTerminal, Read, Write};
use std::net::{IpAddr, TcpListener, UdpSocket};

const DEFAULT_PORT: u16 = 4000;

fn local_ip() -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("192.0.2.1:80")?;
    Ok(socket.local_addr()?.ip())
}

fn usage() -> ! {
    eprintln!("usage: share [-ke] [-p port] [file | -]");
    std::process::exit(1);
}

fn parse_range(request: &str, total: usize) -> Option<(usize, usize)> {
    let line = request.lines().find(|l| l.to_lowercase().starts_with("range:"))?;
    let spec = line.split_once(':')?.1.trim().strip_prefix("bytes=")?;
    let (start_s, end_s) = spec.split_once('-')?;
    let start: usize = start_s.parse().ok()?;
    let end: usize = if end_s.is_empty() { total.saturating_sub(1) } else { end_s.parse().ok()? };
    (start <= end && end < total).then_some((start, end))
}

fn main() -> std::io::Result<()> {
    let mut pargs = pico_args::Arguments::from_env();

    let keep_open = pargs.contains(["-k", "--keep-open"]);
    let embed_video = pargs.contains(["-e", "--embed-video"]);
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

    let (size, unit) = match body.len() as f32 {
        s @ ..1_048_576.0 => (s / 1024.0, "KiB"),
        s => (s / 1024.0 / 1024.0, "MiB"),
    };

    let to_take = if keep_open || embed_video { usize::MAX } else { 1 };
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let mut buf = [0u8; 4096];

    println!("Sharing [{size:.1}{unit}] @ http://{}:{port}", local_ip()?);

    for stream in listener.incoming().take(to_take) {
        let mut stream = stream?;
        println!("\x1b[34m{}\x1b[0m", stream.peer_addr()?);

        let n = stream.read(&mut buf)?;
        let request = String::from_utf8_lossy(&buf[..n]);

        request
            .lines()
            .filter(|h| h.to_lowercase().starts_with("user-agent:"))
            .for_each(|line| println!("\t{line}"));

        let content_type = if embed_video { "Content-Type: video/mp4\r\n" } else { "" };

        let (status, extra_headers, sent): (&str, String, &[u8]) = match parse_range(&request, body.len()) {
            Some((start, end)) => (
                "206 Partial Content",
                format!("Accept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\n", body.len()),
                &body[start..=end],
            ),
            None => (
                "200 OK",
                "Accept-Ranges: bytes\r\n".to_string(),
                body.as_slice(),
            ),
        };

        let header = format!(
            "HTTP/1.1 {status}\r\n{content_type}{extra_headers}Content-Length: {}\r\n\r\n",
            sent.len()
        );

        if let Err(e) = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(sent)) {
            eprintln!("write failed (client likely aborted): {e}");
            continue;
        }
        // stream.write_all(header.as_bytes())?;
        // stream.write_all(sent)?;
    }

    Ok(())
}