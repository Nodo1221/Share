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

enum Request {
    Full,
    Range(usize, usize),
    Bad,
}

impl Request {
    fn parse(raw: &str, total: usize) -> Self {
        let mut lines = raw.lines();

        match lines.next().and_then(|l| l.split_whitespace().nth(1)) {
            Some("/") => {}
            _ => return Request::Bad,
        }

        let mut range = None;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("user-agent") {
                    println!("\t{line}");
                } else if k.eq_ignore_ascii_case("range") {
                    range = (|| {
                        let (start, end) = v.trim().strip_prefix("bytes=")?.split_once('-')?;
                        Some((start.parse().ok()?, end.parse().unwrap_or(total - 1)))
                    })();
                }
            }
        }

        match range {
            Some((start, end)) => Request::Range(start, end),
            None => Request::Full,
        }
    }
}

fn handle_connection<T: Read + Write>(
    mut stream: T,
    buf: &mut [u8],
    body: &[u8],
    embed_video: bool,
) -> std::io::Result<bool> {
    let n = stream.read(buf)?;
    let raw = String::from_utf8_lossy(&buf[..n]);

    let request = Request::parse(&raw, body.len());
    let is_range = matches!(request, Request::Range(..));

    let content_type = if embed_video { "Content-Type: video/mp4\r\n" } else { "" };

    let (status, content_range, payload): (&str, String, &[u8]) = match request {
        Request::Bad => return Ok(false),
        Request::Range(start, end) => (
            "206 Partial Content",
            format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len()),
            &body[start..=end],
        ),
        Request::Full => ("200 OK", String::new(), body),
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\n{content_type}{content_range}Content-Length: {}\r\n\r\n",
        payload.len()
    );
    if let Err(e) = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(payload)) {
        eprintln!("write failed (client likely aborted): {e}");
    }

    Ok(is_range)
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
        0.0 => usage(),
        s @ ..1_048_576.0 => (s / 1024.0, "KiB"),
        s => (s / 1024.0 / 1024.0, "MiB"),
    };

    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let mut buf = [0u8; 4096];

    println!("Sharing [{size:.1}{unit}] @ http://{}:{port}", local_ip()?);

    for stream in listener.incoming() {
        let stream = stream?;
        println!("\x1b[34m{}\x1b[0m", stream.peer_addr()?);
        let is_range_request = handle_connection(stream, &mut buf, &body, embed_video)?;

        if !keep_open && !is_range_request {
            break;
        }
    }

    Ok(())
}