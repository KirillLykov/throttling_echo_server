use {
    anyhow::Result,
    clap::{crate_description, crate_name, crate_version, Parser},
    std::net::SocketAddr,
    throttling_echo_server::{
        configure::configure_server,
        server::{listen, TokenBucketConfig},
    },
    tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver},
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
    let args = Cli::parse();

    let (server_config, _) = configure_server();
    let (sender, receiver) = unbounded_channel();

    tokio::try_join!(
        listen(
            server_config,
            args.listen_address,
            sender,
            TokenBucketConfig::default()
        ),
        consume_data(receiver)
    )?;
    Ok(())
}
