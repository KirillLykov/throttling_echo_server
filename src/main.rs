mod configure;

use {
    anyhow::Result,
    clap::{crate_description, crate_name, crate_version, Parser},
    quinn::{Endpoint, Incoming, ServerConfig},
    std::net::SocketAddr,
    tokio::{
        io::AsyncReadExt,
        sync::mpsc::{self, unbounded_channel, UnboundedReceiver, UnboundedSender},
        time::{self, Duration},
    },
};

type Sender = UnboundedSender<Vec<u8>>;

async fn produce_tokens(
    bytes_per_token: usize,
    token_interval: Duration,
    tokens: mpsc::Sender<usize>,
) -> Result<()> {
    let mut interval = time::interval(token_interval);
    loop {
        interval.tick().await;
        if tokens.send(bytes_per_token).await.is_err() {
            break;
        }
    }
    Ok(())
}

async fn transfer<RecvType>(
    mut from: RecvType,
    mut tokens: mpsc::Receiver<usize>,
    sender: Sender,
) -> Result<()>
where
    RecvType: AsyncReadExt + Unpin,
{
    // TODO try using Bytes: BytesMut::with_capacity(4096);
    const BUF_SIZE: usize = 4 * 1024;
    let mut data = vec![0u8; BUF_SIZE];
    loop {
        let Some(mut bytes_available) = tokens.recv().await else {
            break;
        };

        while bytes_available > 0 {
            let n_read = from
                .read(&mut data[..bytes_available.min(BUF_SIZE)])
                .await?;
            if n_read == 0 {
                eprintln!("Stream finished.");
                break;
            }
            eprintln!("Read chunk: {:?}", &data[..n_read]);
            sender.send(data[..n_read].to_vec())?;
            bytes_available -= n_read;
        }
    }
    Ok(())
}

async fn proxy(incoming: Incoming, sender: Sender) -> Result<()> {
    const NUMBER_OF_TOKENS: usize = 100;
    const BYTES_PER_TOKEN: usize = 20 * 1024;
    const TOKEN_INTERVAL: Duration = Duration::from_millis(100);

    let connection = incoming.await?;

    loop {
        let stream = connection.accept_uni().await;
        let stream = match stream {
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                eprintln!("connection closed");
                return Ok(());
            }
            Err(e) => {
                return Err(e.into());
            }
            Ok(s) => s,
        };

        let (listener_token_sender, listener_token_receiver) = mpsc::channel(NUMBER_OF_TOKENS);

        tokio::try_join!(
            produce_tokens(BYTES_PER_TOKEN, TOKEN_INTERVAL, listener_token_sender),
            transfer(stream, listener_token_receiver, sender.clone()),
        )?;
    }
}

async fn listen(server_config: ServerConfig, listen: SocketAddr, sender: Sender) -> Result<()> {
    let endpoint = Endpoint::server(server_config, listen)?;
    eprintln!("listening on {}", endpoint.local_addr()?);

    while let Some(incoming) = endpoint.accept().await {
        eprintln!("accepting connection");
        tokio::spawn(proxy(incoming, sender.clone()));
    }
    Ok(())
}

async fn consume_data(mut receiver: UnboundedReceiver<Vec<u8>>) -> Result<()> {
    while let Some(data) = receiver.recv().await {
        eprintln!("{data:?}")
    }
    Ok(())
}

#[derive(Parser, Debug, PartialEq, Eq)]
#[clap(name = crate_name!(),
    version = crate_version!(),
    about = crate_description!(),
    rename_all = "kebab-case"
)]
struct Cli {
    #[clap(help = "Listen address")]
    listen_address: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let (server_config, _) = configure::configure_server();
    let (sender, receiver) = unbounded_channel();

    tokio::try_join!(
        listen(server_config, args.listen_address, sender),
        consume_data(receiver)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        configure::{configure_client, configure_server},
        quinn::Endpoint,
        std::net::{Ipv4Addr, SocketAddr},
        tokio::{
            io::AsyncWriteExt,
            sync::mpsc,
            time::{timeout, Duration},
        },
    };

    #[tokio::test]
    async fn test_produce_tokens() {
        let (sender, mut receiver) = mpsc::channel(10);
        let bytes_per_token = 1024;
        let token_interval = Duration::from_millis(10);

        tokio::spawn(async move {
            produce_tokens(bytes_per_token, token_interval, sender)
                .await
                .unwrap();
        });

        for _ in 0..5 {
            let token = receiver.recv().await;
            assert_eq!(token, Some(bytes_per_token));
        }
    }

    #[tokio::test]
    async fn test_transfer() {
        let (mut send_stream, recv_stream) = tokio::io::duplex(64);
        let (token_sender, token_receiver) = mpsc::channel(10);
        let (unbounded_sender, mut unbounded_receiver) = mpsc::unbounded_channel();

        let data = b"hello world";
        send_stream.write_all(data).await.unwrap();

        tokio::spawn(async move {
            transfer(recv_stream, token_receiver, unbounded_sender)
                .await
                .unwrap();
        });

        token_sender.send(1024).await.unwrap();

        let received_data = unbounded_receiver.recv().await.unwrap();
        assert_eq!(received_data, data);
    }

    #[tokio::test]
    async fn test_listen() -> Result<()> {
        let listen_addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let (sender, mut receiver) = mpsc::unbounded_channel();

        let (server_config, server_cert) = configure_server();
        tokio::spawn(async move {
            listen(server_config, listen_addr, sender).await.unwrap();
        });

        // Wait for the server to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Configure client
        let client_config = configure_client(server_cert);

        let bind = SocketAddr::new(Ipv4Addr::new(0, 0, 0, 0).into(), 0);
        let mut endpoint = Endpoint::client(bind)?;
        endpoint.set_default_client_config(client_config);

        // Connect to the server
        let connection = endpoint
            .connect(listen_addr, "localhost")
            .expect("failed to create connecting")
            .await
            .expect("failed to connect");

        // Open a unidirectional stream and send data
        let mut send_stream = connection.open_uni().await.unwrap();
        let data = b"hello world";
        send_stream.write_all(data).await.unwrap();
        send_stream.finish().unwrap();

        // Ensure the server received the data
        let received_data = timeout(Duration::from_secs(1), receiver.recv())
            .await?
            .expect("Data should be received");
        assert_eq!(received_data, data);

        Ok(())
    }
}
