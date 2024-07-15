use {
    anyhow::{anyhow, Result},
    clap::{crate_description, crate_name, crate_version, Args, Parser},
    quinn::{ClientConfig, Endpoint, Incoming, ServerConfig},
    rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    std::{
        io,
        net::{SocketAddr, ToSocketAddrs},
        sync::Arc,
    },
    tokio::{
        sync::mpsc::{self, unbounded_channel, UnboundedReceiver, UnboundedSender},
        time::{self, Duration, Instant},
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

async fn transfer(
    mut from: quinn::RecvStream,
    mut tokens: mpsc::Receiver<usize>,
    sender: Sender,
) -> Result<()> {
    // TODO try using Bytes: BytesMut::with_capacity(4096);
    const BUF_SIZE: usize = 4 * 1024;
    let mut data = vec![0u8; BUF_SIZE];
    loop {
        let Some(mut bytes_available) = tokens.recv().await else {
            break;
        };

        while bytes_available > 0 {
            match from
                .read(&mut data[..bytes_available.min(BUF_SIZE)])
                .await?
            {
                Some(n_read) => {
                    eprintln!("Read chunk: {:?}", &data[..n_read]);
                    sender.send(data[..n_read].to_vec())?;
                    bytes_available -= n_read;
                }
                None => {
                    eprintln!("Stream finished.");
                    break;
                }
            }
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

/// Returns default server configuration along with its certificate.
fn configure_server() -> Result<(ServerConfig, CertificateDer<'static>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = CertificateDer::from(cert.cert);
    let priv_key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let mut server_config =
        ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    Ok((server_config, cert_der))
}

async fn listen(listen: SocketAddr, sender: Sender) -> Result<()> {
    let Ok((server_config, _)) = configure_server() else {
        return Err(anyhow!("This is a custom error message"));
    };
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

    let (sender, receiver) = unbounded_channel();

    tokio::try_join!(listen(args.listen_address, sender), consume_data(receiver))?;
    Ok(())
}
