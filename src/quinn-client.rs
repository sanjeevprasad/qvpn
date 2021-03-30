//! This example demonstrates an HTTP client that requests files from a server.
//!
//! Checkout the `README.md` for guidance.

use std::{
  net::ToSocketAddrs,
  time::{Duration, Instant},
};

use structopt::StructOpt;
use url::Url;

/// HTTP/0.9 over QUIC client
#[derive(StructOpt, Debug)]
#[structopt(name = "client")]
struct Opt {
  url: Url,
  host: Option<String>,
}
pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"h3-29"];

#[tokio::main]
async fn main() {
  let options = Opt::from_args();
  let url = options.url;
  let remote = (url.host_str().unwrap(), url.port().unwrap_or(443))
    .to_socket_addrs()
    .expect("failed to socket addrs")
    .next()
    .expect("couldn't resolve to an address");

  let mut endpoint = quinn::Endpoint::builder();
  let mut client_config = quinn::ClientConfigBuilder::default();
  client_config.protocols(ALPN_QUIC_HTTP);
  endpoint.default_client_config(client_config.build());

  let (endpoint, _incoming) = endpoint
    // .bind(&"[::]:0".parse().unwrap())
    .bind(&"127.0.0.1:0".parse().unwrap())
    .expect("Failed to bind");

  let start = Instant::now();
  let request = format!("GET {} HTTP/3\r\n", url.path());

  // let request = format!("GET {} HTTP/1.1\r\n", url.path());
  let host = options
    .host
    .as_ref()
    .map_or_else(|| url.host_str(), |x| Some(&x))
    .expect("no hostname specified");

  println!("connecting to {} at {}", host, remote);
  let new_conn = match endpoint
    .connect(&remote, &host)
    .expect("failed to connect host err 1")
    .await
  {
    Ok(conn) => conn,
    Err(err) => {
      println!("{}", err);
      std::process::exit(1);
    }
  };

  println!("connected at {:?}", start.elapsed());
  let quinn::NewConnection { connection, .. } = new_conn;
  println!("{}", request);

  let (mut tx, rx) = connection.open_bi().await.expect("failed to open stream");
  // conn.send_datagram(request.into())

  tx.write_all(request.as_bytes())
    .await
    .expect("failed to send request");
  tx.finish().await.expect("failed to shutdown stream");
  let response_start = Instant::now();
  println!("request sent at {:?}", response_start - start);
  //   const SIZE: usize = 1024;
  //   let buf: [u8; SIZE] = [0; SIZE];
  let resp = rx
    .read_to_end(usize::max_value())
    .await
    .expect("failed to read response");
  let duration = response_start.elapsed();
  //   io::stdout().write_all(&resp).unwrap();
  connection.close(0u32.into(), b"done");
  println!("");
  println!(
    "response received in {:?} - {} MiB/s",
    duration,
    resp.len() as f32 / (duration_secs(&duration) * 1024.0 * 1024.0)
  );
  //   io::stdout().flush().unwrap();

  // Give the server a fair chance to receive the close packet
  endpoint.wait_idle().await;
  println!("");
}

fn duration_secs(x: &Duration) -> f32 {
  x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}
