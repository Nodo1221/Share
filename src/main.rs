use clap::{Command, CommandFactory, Parser};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    {ServerConfig, ServerConnection, StreamOwned},
};
use std::net::{IpAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::{
    io::{IsTerminal, Read, Write},
    path::PathBuf,
};

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
                    .and_then(|(start, end)| {
                        Some((start.parse().ok()?, end.parse().unwrap_or(total - 1)))
                    });
            }
        }

        match range {
            Some((start, end)) if end >= total || start > end => Request::Bad,
            Some((start, end)) => Request::Range(start, end),
            None => Request::GetRoot,
        }
    }
}

fn make_tls_config(cert: PathBuf, key: PathBuf) -> Arc<ServerConfig> {
    println!("{cert:?} {key:?}");

    let certs = CertificateDer::pem_file_iter(cert)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let key = PrivateKeyDer::from_pem_file(key).unwrap();

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();

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

    let _ = stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(payload));

    Ok(matches!(request, Request::Range(..)))
}

#[derive(Parser)]
struct Args {
    /// Enable TLS
    #[arg(short, long)]
    tls: bool,

    /// Accept multiple connections
    #[arg(short, long)]
    keep_open: bool,

    /// Enable the built-in video player; auto-enables -k
    #[arg(short, long)]
    embed_video: bool,

    /// Choose a different port
    #[arg(short, long, default_value_t = 4000)]
    port: u16,

    /// Specify the interface to bind to
    #[arg(short, long, default_value_t = "0.0.0.0".to_owned())]
    bind: String,

    /// Specify an alternative PEM pair dir [default: $HOME/.config/share]
    #[arg(long)]
    pem: Option<PathBuf>,

    /// Reads from stdin if omitted
    file_path: Option<PathBuf>,
}

fn main() -> std::io::Result<()> {
    let cmd = Args::command();
    let args = Args::parse();

    let keep_open = args.keep_open || args.embed_video;
    let pem = args.pem.unwrap_or_else(|| {
        home::home_dir().expect("$HOME should be set").join(".config/share")
    });

    let (body, filename) = match args.file_path.as_deref() {
        None if std::io::stdin().is_terminal() => usage(cmd),
        None => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            (buf, "-".to_owned())
        }
        Some(path) => (
            std::fs::read(path)?,
            path.file_name()
                .expect("empty filename handled")
                .to_string_lossy()
                .into_owned(),
        ),
    };

    let (tls, protocol) = match args.tls {
        true => (
            Some(make_tls_config(pem.join("cert.pem"), pem.join("key.pem"))),
            "https",
        ),
        false => (None, "http"),
    };

    let (size, unit) = match body.len() as f32 {
        0.0 => usage(cmd),
        s @ ..1_048_576.0 => (s / 1024.0, "KiB"),
        s => (s / 1024.0 / 1024.0, "MiB"),
    };

    let listener = TcpListener::bind((args.bind, args.port))?;
    let mut buf = [0u8; 4096];

    println!("Sharing {filename} [{size:.1}{unit}] @ {protocol}://{}:{}", local_ip(), args.port);

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Ok(addr) = stream.peer_addr() else { continue };
        println!("\x1b[31m{addr}\x1b[0m");

        let res = match &tls {
            Some(cfg) => match ServerConnection::new(cfg.clone()) {
                Ok(conn) => handle_connection(
                    StreamOwned::new(conn, stream),
                    &mut buf,
                    &body,
                    args.embed_video,
                ),
                Err(_e) => continue,
            },
            None => handle_connection(stream, &mut buf, &body, args.embed_video),
        };

        if !keep_open && matches!(res, Ok(false)) {
            break;
        }
    }

    Ok(())
}
