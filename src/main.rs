use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use rustls::pki_types::{PrivateKeyDer};
use std::sync::Arc;
use std::io::{IsTerminal, Read, Write};
use std::net::{IpAddr, TcpListener, UdpSocket};
use clap::{Command, CommandFactory, Parser};

const DEFAULT_PORT: u16 = 4000;

fn local_ip() -> IpAddr {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("192.0.2.1:80").unwrap();
    socket.local_addr().unwrap().ip()
}

fn usage(mut args: Command) -> ! {
    args.print_help().unwrap();
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

#[derive(Parser)]
struct Args {
    #[arg(short = 't')]
    tls: bool,

    #[arg(short = 'k')]
    keep_open: bool,

    #[arg(short = 'e')]
    embed_video: bool,

    #[arg(short = 'p', default_value_t = DEFAULT_PORT)]
    port: u16,

    file_path: Option<String>,
}

fn main() -> std::io::Result<()> {
    let cmd = Args::command();
    let args = Args::parse();

    let body = match args.file_path.as_deref().unwrap_or("-") {
        "-" if std::io::stdin().is_terminal() => usage(cmd),
        "-" => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            buf
        }
        path => std::fs::read(path)?,
    };

    let tls = match args.tls {
        true => Some(make_tls_config("cert.pem", "key.pem")),
        false => None,
    };

    let (size, unit) = match body.len() as f32 {
        0.0 => usage(cmd),
        s @ ..1_048_576.0 => (s / 1024.0, "KiB"),
        s => (s / 1024.0 / 1024.0, "MiB"),
    };

    let listener = TcpListener::bind(("0.0.0.0", args.port))?;
    let mut buf = [0u8; 4096];

    println!("Sharing [{size:.1}{unit}] @ http://{}:{}", args.port, local_ip());

    for stream in listener.incoming() {
        let stream = stream?;
        println!("\x1b[34m{}\x1b[0m", stream.peer_addr()?);

        let res = match &tls {
            Some(cfg) => {
                match ServerConnection::new(cfg.clone()) {
                    Ok(conn) => handle_connection(StreamOwned::new(conn, stream), &mut buf, &body, args.embed_video),
                    Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
                }
            }
            None => handle_connection(stream, &mut buf, &body, args.embed_video),
        };

        let is_range = match res {
            Ok(r) => r,
            Err(e) => {
                eprintln!("connection error: {e}");
                continue;
            }
        };

        if !args.keep_open && !is_range {
            break;
        }
    }

    Ok(())
}
