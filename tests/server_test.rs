use {
    anyhow::Result,
    quinn::Endpoint,
    std::{
        net::{Ipv4Addr, SocketAddr},
        sync::Once,
    },
    throttling_echo_server::{
        configure::{configure_client, configure_server},
        server::{listen, TokenBucketConfig},
    },
    tokio::{
        sync::mpsc,
        time::{sleep, Duration},
    },
    tokio_util::sync::CancellationToken,
    tracing::info,
    tracing_subscriber::EnvFilter,
};

async fn measure_tps(config: TokenBucketConfig) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    const DATA: &[u8] = &[0; 128];
    let (sender, mut receiver) = mpsc::unbounded_channel();

    let cancel = CancellationToken::new();

    // limit the size of the recv window, otherwise we need to saturate it to see the effect.
    let recv_window_size = DATA.len() as u32 * 16;
    let (mut server_config, server_cert) = configure_server(recv_window_size);
    // I don't see any effect of this:
    server_config.incoming_buffer_size(1);
    let server_endpoint = Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap())?;
    let listen_addr = server_endpoint.local_addr().unwrap();
    let server_handle = {
        let cancel = cancel.clone();
        tokio::spawn(async move { listen(server_endpoint, sender, cancel, config).await })
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client_config = configure_client(server_cert);

    let bind = SocketAddr::new(Ipv4Addr::new(0, 0, 0, 0).into(), 0);
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint
        .connect(listen_addr, "localhost")
        .expect("failed to create connecting")
        .await
        .expect("failed to connect");

    let cancel_timer = tokio::spawn(async move {
        sleep(Duration::from_secs(50)).await;
        cancel.cancel();
    });
    let expected_num_txs = 100;
    let start_time = tokio::time::Instant::now();
    for _ in 0..expected_num_txs {
        // Open a unidirectional stream and send data
        let mut send_stream = connection.open_uni().await.unwrap();
        let data = DATA;
        send_stream.write_all(data).await.unwrap();
        send_stream.finish().unwrap();
    }
    let elapsed_sending: f64 = start_time.elapsed().as_secs_f64();
    info!("Elapsed sending: {elapsed_sending}");

    let start_time = tokio::time::Instant::now();
    let mut elapsed_recv = 0f64;
    let mut total_data_bytes = 0;
    while let Some(received_data) = receiver.recv().await {
        total_data_bytes += received_data.len();
        elapsed_recv = start_time.elapsed().as_secs_f64();
    }
    info!("Elapsed receiving: {elapsed_recv}");

    assert_eq!(expected_num_txs * DATA.len(), total_data_bytes);

    let server_res = server_handle.await;
    assert!(server_res.is_ok(), "Error = {server_res:?}");

    let _ = cancel_timer.await;

    Ok((elapsed_sending, elapsed_recv))
}

static INIT: Once = Once::new();

#[allow(dead_code)]
fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("debug"))
            .init();
    });
}

#[tokio::test]
async fn test_tps_with_very_throttled_connection() {
    init_tracing();
    // the data is 128b, so to receive the packet we need 400ms
    let config = TokenBucketConfig {
        bucket_capacity: 128,
        bytes_per_token: 64,
        token_interval: Duration::from_millis(100),
    };
    let (elapsed_send, elapsed_recv) = measure_tps(config.clone()).await.unwrap();
    println!(
        "Config = {:?}: send time={elapsed_send}, recv time={elapsed_recv}",
        config
    );
}

#[tokio::test]
async fn test_tps_less_throttled() {
    //init_tracing();
    let config = TokenBucketConfig {
        bucket_capacity: 1500 * 100,
        bytes_per_token: 1500,
        token_interval: Duration::from_millis(10),
    };

    let (elapsed_send, elapsed_recv) = measure_tps(config.clone()).await.unwrap();
    println!(
        "Config = {:?}: send time={elapsed_send}, recv time={elapsed_recv}",
        config
    );
}
