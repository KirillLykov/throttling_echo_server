use {
    anyhow::Result,
    clap::{crate_description, crate_name, crate_version, Parser},
    quinn::Endpoint,
    std::net::SocketAddr,
    throttling_echo_server::{
        configure::configure_server,
        server::{listen, TokenBucketConfig},
    },
    tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver},
    tokio_util::sync::CancellationToken,
};

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
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .finish(),
    )
    .unwrap();

    let args = Cli::parse();

    let (server_config, _) = configure_server(1500 * 100);
    let (sender, receiver) = unbounded_channel();

    let cancel = CancellationToken::new(); // we don't really need to cancel ever
    let server_endpoint = Endpoint::server(server_config, args.listen_address)?;
    tokio::try_join!(
        listen(
            server_endpoint,
            sender,
            cancel,
            TokenBucketConfig::default()
        ),
        consume_data(receiver)
    )?;
    Ok(())
}
