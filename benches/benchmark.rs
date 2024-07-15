use {
    criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion},
    quinn::Endpoint,
    std::net::{Ipv4Addr, SocketAddr},
    throttling_echo_server::{
        configure::{configure_client, configure_server},
        server::{listen, TokenBucketConfig},
    },
    tokio::sync::mpsc,
    tokio::time::{timeout, Duration},
};

// Function to measure the performance of the server with given TokenBucketConfig
async fn measure_tps(config: TokenBucketConfig) -> Result<f64, Box<dyn std::error::Error>> {
    let (sender, mut receiver) = mpsc::unbounded_channel();

    let (server_config, server_cert) = configure_server();
    let server_endpoint = Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap())?;
    let listen_addr = server_endpoint.local_addr().unwrap();
    tokio::spawn(async move {
        listen(server_endpoint, sender, config).await.unwrap();
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

    let start_time = tokio::time::Instant::now();
    let mut tps = 0;

    for _ in 0..100 {
        let mut send_stream = connection.open_uni().await.unwrap();
        let data = b"hello world";
        send_stream.write_all(data).await.unwrap();
        send_stream.finish().unwrap();

        let received_data = timeout(Duration::from_secs(1), receiver.recv())
            .await?
            .expect("Data should be received");

        if received_data == data {
            tps += 1;
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    Ok(tps as f64 / elapsed)
}

// Benchmark function
fn benchmark_tps(c: &mut Criterion) {
    let configs = vec![
        TokenBucketConfig {
            bucket_capacity: 1500 * 60,
            bytes_per_token: 1500,
            token_interval: Duration::from_millis(100),
        },
        TokenBucketConfig {
            bucket_capacity: 1500 * 60 * 5,
            bytes_per_token: 1500,
            token_interval: Duration::from_millis(100),
        },
        TokenBucketConfig {
            bucket_capacity: 1500 * 60,
            bytes_per_token: 1500,
            token_interval: Duration::from_millis(50),
        },
    ];

    let mut group = c.benchmark_group("TPS Benchmark");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(120));

    for config in configs {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!(
                "BucketCapacity_{}_BytesPerToken_{}_TokenInterval_{:?}",
                config.bucket_capacity, config.bytes_per_token, config.token_interval
            )),
            &config,
            |b, &config| {
                b.to_async(tokio::runtime::Runtime::new().unwrap())
                    .iter(|| measure_tps(black_box(config.clone())));
            },
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_tps);
criterion_main!(benches);
