use {
    rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    std::{convert::TryInto, sync::Arc, time::Duration},
};

/// Builds client configuration. Trusts given node certificate.
#[cfg(test)]
pub fn configure_client(node_cert: CertificateDer<'static>) -> quinn::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(node_cert).unwrap();

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_idle_timeout(Some(Duration::from_secs(20).try_into().unwrap()));

    let mut peer_cfg = quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap();
    peer_cfg.transport_config(Arc::new(transport_config));
    peer_cfg
}

/// Builds listener configuration along with its certificate.
pub fn configure_server() -> (quinn::ServerConfig, CertificateDer<'static>) {
    let (our_cert, our_priv_key) = gen_cert();
    let mut our_cfg =
        quinn::ServerConfig::with_single_cert(vec![our_cert.clone()], our_priv_key.into()).unwrap();

    let transport_config = Arc::get_mut(&mut our_cfg.transport).unwrap();
    transport_config.max_idle_timeout(Some(Duration::from_secs(20).try_into().unwrap()));

    (our_cfg, our_cert)
}

fn gen_cert() -> (CertificateDer<'static>, PrivatePkcs8KeyDer<'static>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (
        cert.cert.into(),
        PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()),
    )
}
