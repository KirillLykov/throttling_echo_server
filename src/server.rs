use {
    anyhow::Result,
    quinn::{Endpoint, Incoming},
    tokio::{
        io::AsyncReadExt,
        sync::mpsc::{self, UnboundedSender},
        time::{self, Duration},
    },
    tokio_util::sync::CancellationToken,
    tracing::{debug, error, info},
};

type Sender = UnboundedSender<Vec<u8>>;

#[derive(Debug, Clone, Copy)]
pub struct TokenBucketConfig {
    pub bucket_capacity: usize,
    pub bytes_per_token: usize,
    pub token_interval: Duration,
}

impl Default for TokenBucketConfig {
    fn default() -> Self {
        TokenBucketConfig {
            bucket_capacity: 100,
            bytes_per_token: 1500,
            token_interval: Duration::from_millis(100),
        }
    }
}

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
    tokens: &mut mpsc::Receiver<usize>,
    sender: Sender,
) -> Result<()>
where
    RecvType: AsyncReadExt + Unpin,
{
    // TODO try using Bytes: BytesMut::with_capacity(4096);
    const BUF_SIZE: usize = 4 * 1024;
    let mut data = vec![0u8; BUF_SIZE];
    'outer: loop {
        let Some(mut bytes_available) = tokens.recv().await else {
            break;
        };

        while bytes_available > 0 {
            let n_read = from
                .read(&mut data[..bytes_available.min(BUF_SIZE)])
                .await?;
            if n_read == 0 {
                debug!("Stream finished.");
                break 'outer;
            }
            debug!("Read bytes: {n_read}");
            let _ = sender.send(data[..n_read].to_vec())?;
            bytes_available -= n_read;
        }
    }
    Ok(())
}

async fn proxy(
    incoming: Incoming,
    sender: Sender,
    cancel: CancellationToken,
    config: TokenBucketConfig,
) -> Result<()> {
    let TokenBucketConfig {
        bucket_capacity,
        bytes_per_token,
        token_interval,
    } = config;
    let (listener_token_sender, mut listener_token_receiver) = mpsc::channel(bucket_capacity);
    let h = tokio::spawn(produce_tokens(
        bytes_per_token,
        token_interval,
        listener_token_sender,
    ));
    let connection = incoming.await?;
    loop {
        if cancel.is_cancelled() {
            info!("stopping connection handling due to signal received.");
            break;
        }
        let stream = connection.accept_uni().await;
        let stream = match stream {
            Err(quinn::ConnectionError::ApplicationClosed(e)) => {
                info!("connection closed: {e:?}");
                break;
            }
            Err(e) => {
                return Err(e.into());
            }
            Ok(s) => s,
        };

        debug!("Got stream");

        transfer(stream, &mut listener_token_receiver, sender.clone()).await?;
    }
    drop(listener_token_receiver);
    let _ = h.await?;
    Ok(())
}

pub async fn listen(
    endpoint: Endpoint,
    sender: Sender,
    cancel: CancellationToken,
    config: TokenBucketConfig,
) -> Result<()> {
    info!("listening on {}", endpoint.local_addr()?);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Shutting down.");
                break;
            },
            Some(incoming) = endpoint.accept() => {
                if sender.is_closed() {
                    error!("Sender is unexpectedly closed.");
                    break;
                }
                info!("accepting connection");
                tokio::spawn(proxy(incoming, sender.clone(), cancel.clone(), config));
            },
            else => {
                error!("Endpoint is unexpectedly closed.");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::configure::{configure_client, configure_server},
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
        let (token_sender, mut token_receiver) = mpsc::channel(10);
        let (unbounded_sender, mut unbounded_receiver) = mpsc::unbounded_channel();

        let data = b"hello world";
        send_stream.write_all(data).await.unwrap();

        tokio::spawn(async move {
            transfer(recv_stream, &mut token_receiver, unbounded_sender)
                .await
                .unwrap();
        });

        token_sender.send(1024).await.unwrap();

        let received_data = unbounded_receiver.recv().await.unwrap();
        assert_eq!(received_data, data);
    }

    #[tokio::test]
    async fn test_listen() -> Result<()> {
        let (sender, mut receiver) = mpsc::unbounded_channel();

        let (server_config, server_cert) = configure_server(1500);
        let server_endpoint = Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap())?;
        let listen_addr = server_endpoint.local_addr().unwrap();
        let cancel = CancellationToken::new();
        tokio::spawn(async move {
            listen(
                server_endpoint,
                sender,
                cancel,
                TokenBucketConfig::default(),
            )
            .await
            .unwrap();
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
