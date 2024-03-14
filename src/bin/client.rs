use a_chat::Result;

use async_std::io::{stdin, BufReader};
use async_std::net::{TcpStream, ToSocketAddrs};
use async_std::prelude::*;
use async_std::task;
use futures::{select, FutureExt};

fn main() {
    if let Err(e) = run() {
        eprint!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    task::block_on(try_run("127.0.0.1:6060"))
}

async fn try_run(addr: impl ToSocketAddrs) -> Result<()> {
    let stream = TcpStream::connect(addr).await?;
    let (reader, mut writer) = (&stream, &stream);
    let mut lines_from_server = BufReader::new(reader).lines().fuse();
    let mut lines_from_stdin = BufReader::new(stdin()).lines().fuse();
    loop {
        select! {
            line = lines_from_server.next().fuse() => match line {
                Some(line) => {
                    println!("{}", line?);
                },
                None => break,
            },
            line = lines_from_stdin.next().fuse() => match line {
                Some(line) => {
                    let line = line?;
                    writer.write_all(line.as_bytes()).await?;
                    writer.write_all(b"\n").await?;

                },
                None => break,
            }
        }
    }
    Ok(())
}
