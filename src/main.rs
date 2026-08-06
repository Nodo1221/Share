use clap::{Command, CommandFactory, Parser};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use std::net::{IpAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::{
    fs::File,
    io::{ErrorKind, IsTerminal, BufReader, BufRead, Read, Write, BufWriter},
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
    Post { boundary: String },
    Bad,
}

impl Request {
    fn parse(raw: &str, total: usize) -> Self {
        let mut lines = raw.lines();
        let Some(first) = lines.next() else { return Request::Bad };

        let is_post = first.starts_with("POST /upload ");
        if !is_post && !first.starts_with("GET / ") {
            return Request::Bad;
        }

        let mut range = None;
        let mut boundary = None;

        for line in lines {
            let Some((key, val)) = line.split_once(':') else { continue };
            let val = val.trim();

            if key.eq_ignore_ascii_case("user-agent") {
                println!("\t{line}");
            } else if key.eq_ignore_ascii_case("range") {
                range = val.strip_prefix("bytes=")
                    .and_then(|r| r.split_once('-'))
                    .and_then(|(start, end)| Some((start.parse().ok()?, end.parse().unwrap_or(total - 1))));
            } else if key.eq_ignore_ascii_case("content-type") {
                boundary = val.split_once("boundary=").map(|(_, b)| b.to_owned());
            }
        }

        if is_post {
            return match boundary {
                Some(boundary) => Request::Post { boundary },
                None => Request::Bad,
            };
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

fn strip_crlf(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r\n").or(line.strip_suffix(b"\n")).unwrap_or(line)
}

fn handle_post(reader: &mut BufReader<impl Read>, boundary: &str) {
    let delim = format!("--{boundary}").into_bytes();
    let end = format!("--{boundary}--").into_bytes();
    let mut line = Vec::new();

    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).unwrap_or(0) == 0 { break }
        if strip_crlf(&line) == end { break }
        if strip_crlf(&line) != delim { continue }

        let mut filename = None;
        loop {
            line.clear();
            reader.read_until(b'\n', &mut line).expect("client already promised this via Content-Length");
            if strip_crlf(&line).is_empty() { break }

            let header = String::from_utf8_lossy(&line);
            if let Some(name) = header.split("filename=\"").nth(1).and_then(|s| s.split('"').next()) {
                filename = Some(name.to_string());
            }
        }

        let Some(filename) = filename else { continue };
        let mut out = BufWriter::new(File::create(&filename).expect("upload dir should be writable"));

        let mut pending: Option<Vec<u8>> = None;
        let is_end;
        loop {
            let mut buf = Vec::new();
            reader.read_until(b'\n', &mut buf).expect("connection shouldn't drop mid-upload");
            let t = strip_crlf(&buf);

            if t == delim || t == end {
                is_end = t == end;
                if let Some(p) = pending.take() {
                    out.write_all(strip_crlf(&p)).unwrap();
                }
                break;
            }
            if let Some(p) = pending.replace(buf) {
                out.write_all(&p).unwrap();
            }
        }
        out.flush().unwrap();
        println!("\tsaved {filename}");
        if is_end { break }
    }
}

fn handle_connection(
    stream: impl Read + Write,
    body: &[u8],
    embed_video: bool,
) -> std::io::Result<bool> {
    let mut reader = BufReader::new(stream);

    let mut raw = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 { break }
        if line == "\r\n" || line == "\n" { break }
        raw.push_str(&line);
    }

    let request = Request::parse(&raw, body.len());
    let content_type = if embed_video { "Content-Type: video/mp4\r\n" } else { "" };

    if let Request::Post { boundary } = &request {
        handle_post(&mut reader, boundary);
        let _ = reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        return Ok(false);
    }

    let (status, content_range, payload) = match request {
        Request::GetRoot => ("200 OK", String::new(), body),
        Request::Range(start, end) => (
            "206 Partial Content",
            format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len()),
            &body[start..=end],
        ),
        Request::Bad | Request::Post { .. } => return Ok(false),
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\n{content_type}{content_range}Content-Length: {}\r\n\r\n",
        payload.len()
    );

    let _ = reader
        .get_mut()
        .write_all(header.as_bytes())
        .and_then(|_| reader.get_mut().write_all(payload));

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

    /// Choose a different port; 0 for random
    #[arg(short, long, default_value_t = 4000)]
    port: u16,

    /// Specify the interface to bind to
    #[arg(short, long, default_value_t = "0.0.0.0".to_owned())]
    bind: String,

    /// Specify an alternative PEM pair dir [default: $HOME/.config/share]
    #[arg(long)]
    pem: Option<PathBuf>,

    #[arg(short = 'u', long)]
    accept_uploads: bool,

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
    let path = if !args.accept_uploads { args.file_path } else { Some(PathBuf::from(home::home_dir().unwrap().join(".config/share/share.html"))) };
    println!("{path:?}");

    let (body, filename) = match path.as_deref() {
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

    let listener = match TcpListener::bind((args.bind.as_ref(), args.port)) {
        Ok(l) => l,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            eprintln!("\x1b[33mwarning: address already in use; falling back to a randomised port\x1b[0m");
            TcpListener::bind((args.bind, 0))?
        }
        Err(e) => return Err(e),
    };
    
    let port = listener.local_addr()?.port();

    println!("Sharing {filename} [{size:.1}{unit}] @ {protocol}://{}:{port}", local_ip());

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Ok(addr) = stream.peer_addr() else { continue };
        println!("\x1b[31m{addr}\x1b[0m");

        let res = match &tls {
            Some(cfg) => match ServerConnection::new(cfg.clone()) {
                Ok(conn) => handle_connection(
                    StreamOwned::new(conn, stream),
                    &body,
                    args.embed_video,
                ),
                Err(_e) => continue,
            },
            None => handle_connection(stream, &body, args.embed_video),
        };

        if !keep_open && matches!(res, Ok(false)) {
            break;
        }
    }

    Ok(())
}
