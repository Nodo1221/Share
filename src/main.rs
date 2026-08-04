use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use rustls::pki_types::{PrivateKeyDer};
use std::sync::Arc;
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
    GetRoot,
    Range(usize, usize),
    Bad,
}

impl Request {
    fn parse(raw: &str, total: usize) -> Self {
        let mut lines = raw.lines();

        if !lines.next().is_some_and(|l| l.starts_with("GET / ")) {
            return Request::Bad;
        }

        let mut range = None;
        for line in lines {
            let Some((key, val)) = line.split_once(':') else { continue };
        
            if key.eq_ignore_ascii_case("user-agent") {
                println!("\t{line}");
            }

            else if key.eq_ignore_ascii_case("range") {
                range = val.trim()
                    .strip_prefix("bytes=")
                    .and_then(|r| r.split_once('-'))
                    .and_then(|(start, end)| Some((start.parse().ok()?, end.parse().unwrap_or(total - 1))));
            }
        }

        match range {
            Some((start, end)) if end >= total || start > end => Request::Bad,
            Some((start, end)) => Request::Range(start, end),
            None => Request::GetRoot,
        }
    }
}

fn make_tls_config(cert: &str, key: &str) -> Arc<ServerConfig> {
    let certs = CertificateDer::pem_file_iter(cert)
        .expect("failed to open certificate file")
        .collect::<Result<Vec<_>, _>>()
        .expect("failed to parse certificate");

    let key = PrivateKeyDer::from_pem_file(key)
        .expect("failed to load or parse private key");

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("failed to build TLS configuration");

    Arc::new(config)
}

fn handle_connection(
    mut stream: impl Read + Write,
    buf: &mut [u8],
    body: &[u8],
    embed_video: bool,
) -> std::io::Result<bool> {
    let n = stream.read(buf)?;
    let raw = String::from_utf8_lossy(&buf[..n]);
    let request = Request::parse(&raw, body.len());
    let content_type = if embed_video { "Content-Type: video/mp4\r\n" } else { "" };

    let (status, content_range, payload) = match request {
        Request::GetRoot => ("200 OK", String::new(), body),
        Request::Range(start, end) => (
            "206 Partial Content",
            format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len()),
            &body[start..=end],
        ),
        Request::Bad => return Ok(false),
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\n{content_type}{content_range}Content-Length: {}\r\n\r\n",
        payload.len()
    );
    
    if let Err(e) = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(payload)) {
        eprintln!("write failed (client likely aborted): {e}");
    }

    Ok(matches!(request, Request::Range(..)))
}

fn main() -> std::io::Result<()> {
    let mut pargs = pico_args::Arguments::from_env();
    let keep_open = pargs.contains(["-k", "--keep-open"]);
    let embed_video = pargs.contains(["-e", "--embed-video"]);
    let port: u16 = pargs
        .opt_value_from_str(["-p", "--port"])
        .unwrap_or_else(|_| usage())
        .unwrap_or(DEFAULT_PORT);
    let cert: Option<String> = pargs
        .opt_value_from_str(["-c", "--cert"])
        .unwrap_or_else(|_| usage());
    let key: Option<String> = pargs
        .opt_value_from_str(["-K", "--key"])
        .unwrap_or_else(|_| usage());
    let file_path: Option<String> = pargs.opt_free_from_str().unwrap_or_else(|_| usage());

    let tls = match (cert, key) {
        (Some(c), Some(k)) => Some(make_tls_config(&c, &k)?),
        (None, None) => None,
        _ => usage(),
    };

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

        let res = match &tls {
            Some(cfg) => {
                match ServerConnection::new(cfg.clone()) {
                    Ok(conn) => handle_connection(StreamOwned::new(conn, stream), &mut buf, &body, embed_video),
                    Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
                }
            }
            None => handle_connection(stream, &mut buf, &body, embed_video),
        };

        let is_range = match res {
            Ok(r) => r,
            Err(e) => {
                eprintln!("connection error: {e}");
                continue;
            }
        };

        if !keep_open && !is_range {
            break;
        }
    }

    Ok(())
}
