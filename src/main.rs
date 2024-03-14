use a_chat::Result;
use async_std::net::{TcpListener, TcpStream, ToSocketAddrs};
use async_std::prelude::*;
use async_std::task;
use futures::{select, FutureExt};
use std::collections::HashMap;
use std::sync::Arc;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    task::block_on(accept_loop("127.0.0.1:6060"))
}

async fn accept_loop(addr: impl ToSocketAddrs) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;

    let (broker_sender, broker_receiver) = mpsc::unbounded();
    let broker_handle = task::spawn(broker_loop(broker_receiver));
    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        let stream = stream?;
        println!("Accepting from: {}", stream.peer_addr()?);
        spawn_and_log_error(connection_loop(broker_sender.clone(), stream));
    }

    drop(broker_sender);
    broker_handle.await;
    Ok(())
}

use async_std::io::BufReader;

async fn connection_loop(mut broker: Sender<Event>, stream: TcpStream) -> Result<()> {
    let stream = Arc::new(stream);
    let reader = BufReader::new(&*stream);
    let mut lines = reader.lines();

    let name = match lines.next().await {
        None => Err("peer disconnected immediately")?,
        Some(name) => Arc::new(name?),
    };
    println!("name = {}", name);

    let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<Void>();

    broker
        .send(Event::NewPeer {
            name: Arc::clone(&name),
            stream: Arc::clone(&stream),
            shutdown: shutdown_receiver,
        })
        .await
        .unwrap();

    while let Some(line) = lines.next().await {
        let line = line?;
        let (dest, msg) = match line.find(':') {
            None => continue,
            Some(idx) => (&line[..idx], line[idx + 1..].trim()),
        };
        let dest = dest
            .split(',')
            .map(|name| name.trim().to_string())
            .collect::<Vec<_>>();
        let msg = msg.to_string();

        println!("send msg {:?} to {}", msg, dest.join(", "));

        broker
            .send(Event::Message {
                from: Arc::clone(&name),
                to: dest,
                msg,
            })
            .await
            .unwrap();
    }
    Ok(())
}

use futures::channel::mpsc;
use futures::sink::SinkExt;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

async fn connection_write_loop(
    messages: &mut Receiver<Arc<String>>,
    stream: Arc<TcpStream>,
    shutdown: Receiver<Void>,
) -> Result<()> {
    let mut stream = &*stream;
    let mut messages = messages.fuse();
    let mut shutdown = shutdown.fuse();

    loop {
        select! {
            msg = messages.next().fuse() => match msg {
                Some(msg) => stream.write_all(msg.as_bytes()).await?,
                None => break,
            },
            void = shutdown.next().fuse() => match void {
                Some(void) => match void {},
                None => break,
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
enum Void {}

#[derive(Debug)]
enum Event {
    NewPeer {
        name: Arc<String>,
        stream: Arc<TcpStream>,
        shutdown: Receiver<Void>,
    },

    Message {
        from: Arc<String>,
        to: Vec<String>,
        msg: String,
    },
}

async fn broker_loop(events: Receiver<Event>) {
    let (disconnect_sender, mut disconnect_receiver) =
        mpsc::unbounded::<(Arc<String>, Receiver<Arc<String>>)>();
    let mut peers: HashMap<Arc<String>, Sender<Arc<String>>> = HashMap::new();
    let mut events = events.fuse();

    loop {
        let event = select! {
            event = events.next().fuse() => match event {
                None => break,
                Some(event) => event,

            },
            disconnect = disconnect_receiver.next().fuse() => {
                let (name, _pending_messages) = disconnect.unwrap();
                assert!(peers.remove(&name).is_some());
                continue;
            }
        };

        match event {
            Event::Message { from, to, msg } => {
                let msg = Arc::new(format!("from {}: {}\n", from, msg));

                for addr in to {
                    if let Some(peer) = peers.get_mut(&addr) {
                        peer.send(Arc::clone(&msg)).await.unwrap();
                    }
                }
            }
            Event::NewPeer {
                name,
                stream,
                shutdown,
            } => {
                peers.entry(Arc::clone(&name)).or_insert_with(|| {
                    let (sender, mut receiver) = mpsc::unbounded();
                    let mut disconnect_sender = disconnect_sender.clone();
                    spawn_and_log_error(async move {
                        let res = connection_write_loop(&mut receiver, stream, shutdown).await;
                        disconnect_sender
                            .send((Arc::clone(&name), receiver))
                            .await
                            .unwrap();
                        res
                    });
                    sender
                });
            }
        }
    }

    drop(peers);
    drop(disconnect_sender);
    while disconnect_receiver.next().await.is_some() {}
}

fn spawn_and_log_error<F>(fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    task::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("{}", e);
        }
    })
}
