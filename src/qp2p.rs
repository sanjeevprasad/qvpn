use bytes::Bytes;
use qp2p::{Config, Endpoint, QuicP2p};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::{env, sync::Arc};
use tokio::{io::AsyncReadExt, sync::Mutex};
#[tokio::main]
async fn main() -> ! {
  // collect cli args
  println!("-----------------------------------------------------------------");
  let args: Vec<String> = env::args().collect();

  // instantiate QuicP2p with custom config
  let qp2p = QuicP2p::with_config(
    Some(Config {
      local_ip: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
      // external_ip: Some(IpAddr::V4(Ipv4Addr::from([0,0,0,0]))),
      idle_timeout_msec: Some(1000 * 3600), // 1 hour idle timeout.
      ..Default::default()
    }),
    Default::default(),
    true,
  )
  .expect("qp2p creation failed");

  // create an endpoint for us to listen on and send from.
  let (node, mut incoming_conns, mut incoming_messages, mut disconnections) =
    qp2p.new_endpoint().await.expect("qp2p endpoint failed");

  let mut peers_list: Vec<SocketAddr> = vec![];
  let server_mode = if args.len() > 1 {
    for arg in args.iter().skip(1) {
      let peer: SocketAddr = arg
        .parse()
        .expect("Invalid SocketAddr.  Use the form 127.0.0.1:1234");
      println!("Connecting... {}", peer);
      if let Err(err) = node.connect_to(&peer).await {
        panic!("{} {:?}", err, err);
      }
      peers_list.push(peer);
    }
    ""
  } else {
    " (Server Mode)"
  };
  let peers_list = Arc::new(Mutex::new(peers_list));
  let peers = peers_list.clone();
  tokio::spawn(async move {
    loop {
      match incoming_conns.next().await {
        None => panic!("incoming no connection breaking;"),
        Some(peer) => {
          println!("incoming {}", peer);
          peers.lock().await.push(peer);
        }
      }
    }
  });

  let peers = peers_list.clone();
  tokio::spawn(async move {
    loop {
      match disconnections.next().await {
        None => panic!("disconnection no connection breaking;"),
        Some(peer) => {
          println!("disconnected {}", peer);
          peers.lock().await.push(peer);
        }
      }
    }
  });

  println!("Listening on: {:?}{}", node.socket_addr(), server_mode);
  println!("Listening on: {:?}{}", node.local_addr(), server_mode);
  println!("-----------------------------------------------------------------");
  let peers = peers_list.clone();
  // loop over incoming messages

  let endpoint = Arc::new(Mutex::new(node));
  let node = endpoint.clone();
  tokio::spawn(async move {
    let mut stdin = tokio::io::stdin();
    const SIZE: usize = 10;
    let mut buf: [u8; SIZE] = [0; SIZE];
    loop {
      match stdin.read(&mut buf).await {
        Ok(len) => {
          let buf = &buf[0..len];
          let msg = Bytes::from(buf.to_owned());
          send_to_all(&node, &peers, msg).await;
        }
        Err(err) => {
          println!("{:?}", err);
          break;
        }
      }
    }
  });
  let node = endpoint.clone();
  let len = peers_list.lock().await.len();
  println!("peers: {}", len);
  let msg_hi: Bytes = Bytes::from("Hi");
  let msg_hello: Bytes = Bytes::from("Hello");
  if len > 0 {
    send_to_all(&node, &peers_list, msg_hi.clone()).await;
  }
  loop {
    match incoming_messages.next().await {
      None => std::process::exit(1),
      Some((peer, bytes)) => {
        println!("<-- {:?} : {:?}", peer, bytes);
        if bytes == msg_hi {
          let node = node.lock().await;
          println!("-->                 : {:?}", msg_hello);
          node
            .send_message(msg_hello.clone(), &peer)
            .await
            .expect("send message failed");
        }
      }
    }
  }
}

pub async fn send_to_all<'a>(
  node: &Arc<Mutex<Endpoint>>,
  peers: &Arc<Mutex<Vec<SocketAddr>>>,
  msg: Bytes,
) {
  let peers = peers.lock().await;
  let locked_node = node.lock().await;
  println!("-->                 : {:?}", msg);
  for peer in peers.iter() {
    locked_node
      .send_message(msg.to_owned(), &peer)
      .await
      .expect("send_to_all failed");
  }
}
