//! This example demonstrates an HTTP server that serves files from a directory.
//!
//! Checkout the `README.md` for guidance.

use std::{
  ascii, fs, io,
  net::SocketAddr,
  path::{self, Path, PathBuf},
  str,
  sync::Arc,
};

use futures::StreamExt;
use structopt::{self, StructOpt};
use tokio::io::{AsyncReadExt, BufReader};

#[derive(StructOpt, Debug)]
#[structopt(name = "server")]
struct Opt {
  /// file to log TLS keys to for debugging
  #[structopt(long = "keylog")]
  keylog: bool,
  /// directory to serve files from
  #[structopt(parse(from_os_str))]
  root: PathBuf,
  /// TLS private key in PEM format
  #[structopt(parse(from_os_str), short = "k", long = "key", requires = "cert")]
  key: Option<PathBuf>,
  /// TLS certificate in PEM format
  #[structopt(parse(from_os_str), short = "c", long = "cert", requires = "key")]
  cert: Option<PathBuf>,
  /// Enable stateless retries
  #[structopt(long = "stateless-retry")]
  stateless_retry: bool,
  /// Address to listen on
  //   #[structopt(long = "listen", default_value = "[::1]:4433")]
  #[structopt(long = "listen", default_value = "127.0.0.1:4433")]
  listen: SocketAddr,
}

pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"h3-29"];

#[tokio::main]
#[allow(clippy::field_reassign_with_default)] // https://github.com/rust-lang/rust-clippy/issues/6527
async fn main() -> ! {
  let options = Opt::from_args();
  let mut transport_config = quinn::TransportConfig::default();
  transport_config.max_concurrent_uni_streams(0).unwrap();
  let mut server_config = quinn::ServerConfig::default();
  server_config.transport = Arc::new(transport_config);
  let mut server_config = quinn::ServerConfigBuilder::new(server_config);
  server_config.protocols(ALPN_QUIC_HTTP);

  if options.keylog {
    server_config.enable_keylog();
  }

  if options.stateless_retry {
    server_config.use_stateless_retry(true);
  }

  if let (Some(key_path), Some(cert_path)) = (&options.key, &options.cert) {
    let key = fs::read(key_path).unwrap();
    let key = if key_path.extension().map_or(false, |x| x == "der") {
      quinn::PrivateKey::from_der(&key).unwrap()
    } else {
      quinn::PrivateKey::from_pem(&key).unwrap()
    };
    let cert_chain = fs::read(cert_path).unwrap();
    let cert_chain = if cert_path.extension().map_or(false, |x| x == "der") {
      quinn::CertificateChain::from_certs(quinn::Certificate::from_der(&cert_chain))
    } else {
      quinn::CertificateChain::from_pem(&cert_chain).unwrap()
    };
    server_config.certificate(cert_chain, key).unwrap();
  } else {
    let dirs = directories_next::ProjectDirs::from("org", "quinn", "quinn-examples").unwrap();
    let path = dirs.data_local_dir();
    let cert_path = path.join("cert.der");
    let key_path = path.join("key.der");
    let (cert, key) = match fs::read(&cert_path).and_then(|x| Ok((x, fs::read(&key_path).unwrap())))
    {
      Ok(x) => x,
      Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
        println!("generating self-signed certificate");
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let key = cert.serialize_private_key_der();
        let cert = cert.serialize_der().unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(&cert_path, &cert).unwrap();
        fs::write(&key_path, &key).unwrap();
        (cert, key)
      }
      Err(e) => {
        panic!("failed to read certificate: {}", e);
      }
    };
    let key = quinn::PrivateKey::from_der(&key).unwrap();
    let cert = quinn::Certificate::from_der(&cert).unwrap();
    server_config
      .certificate(quinn::CertificateChain::from_certs(vec![cert]), key)
      .unwrap();
  }

  let mut endpoint = quinn::Endpoint::builder();
  endpoint.listen(server_config.build());

  let root = Arc::<Path>::from(options.root.clone());
  if !root.exists() {
    panic!("root path does not exist");
  }

  let (endpoint, mut incoming) = endpoint.bind(&options.listen).unwrap();
  eprintln!("listening on {}", endpoint.local_addr().unwrap());

  while let Some(conn) = incoming.next().await {
    println!("connection incoming");
    tokio::spawn(handle_connection(root.clone(), conn));
  }
  std::process::exit(1);
}

async fn handle_connection(root: Arc<Path>, conn: quinn::Connecting) {
  let quinn::NewConnection { mut bi_streams, .. } = match conn.await {
    Ok(conn) => conn,
    Err(err) => {
      println!("{} {:?}", err, err);
      return;
    }
  };
  println!("established");

  // Each stream initiated by the client constitutes a new request.
  while let Some(stream) = bi_streams.next().await {
    let stream = match stream {
      Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
        println!("connection closed");
        break;
      }
      Err(e) => {
        println!("{:?}", e);
        break;
      }
      Ok(s) => s,
    };
    tokio::spawn(handle_request(root.clone(), stream));
  }
}

async fn handle_request(
  root: Arc<Path>,
  (mut response_stream, recv): (quinn::SendStream, quinn::RecvStream),
) {
  let req = recv
    .read_to_end(64 * 1024)
    .await
    .map_err(|e| panic!("failed reading request: {}", e))
    .unwrap();
  let mut escaped = String::new();
  for &x in &req[..] {
    let part = ascii::escape_default(x).collect::<Vec<_>>();
    escaped.push_str(str::from_utf8(&part).unwrap());
  }
  println!("content: {}", escaped);
  // Execute the request
  let x = &req;
  if x.len() < 4 || &x[0..4] != b"GET " {
    panic!("missing GET");
  }
  if x[4..].len() < 2 || &x[x.len() - 2..] != b"\r\n" {
    panic!("missing \\r\\n");
  }
  let x = &x[4..x.len() - 2];
  let end = x.iter().position(|&c| c == b' ').unwrap_or_else(|| x.len());
  let path = str::from_utf8(&x[..end]).unwrap();
  let path = Path::new(&path);
  let mut real_path = PathBuf::from(&root as &Path);
  let mut components = path.components();
  match components.next() {
    Some(path::Component::RootDir) => {}
    _ => panic!("path must be absolute"),
  }
  for c in components {
    match c {
      path::Component::Normal(x) => {
        real_path.push(x);
      }
      x => {
        panic!("illegal component in path: {:?}", x);
      }
    }
  }
  let file = match tokio::fs::File::open(&real_path).await {
    Ok(file) => file,
    Err(err) => {
      println!("{}", err);
      response_stream
        .write_all(b"HTTP/3 404 NotFound\r\n")
        .await
        .map_err(|e| panic!("failed to send response: {}", e))
        .unwrap();
      response_stream
        .finish()
        .await
        .map_err(|e| panic!("failed to shutdown stream: {}", e))
        .unwrap();
      return;
    }
  };
  const SIZE: usize = 1024 * 100;
  let mut buf: [u8; SIZE] = [0; SIZE];

  let mut reader = BufReader::new(file);
  let mut i: usize = 0;
  while let Ok(len) = reader.read_exact(&mut buf).await {
    println!("{} MB", i * SIZE / 1024 / 1024);
    i = i + 1;
    response_stream
      .write(&buf[0..len])
      .await
      .map_err(|e| panic!("failed to response_stream response: {}", e))
      .unwrap();
  }
  // Gracefully terminate the stream
  // reader.write(response_stream);
  response_stream
    .finish()
    .await
    .map_err(|e| panic!("failed to shutdown stream: {}", e))
    .unwrap();
  println!("complete");
}
