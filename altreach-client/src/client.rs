use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use quinn::{ClientConfig, Endpoint, RecvStream, SendStream};
use rustls::pki_types::{ServerName, UnixTime};
use altreach_proto::{ClientMessage, ServerMessage, encode, decode};

#[derive(Debug)]
struct SkipVerification;

impl rustls::client::danger::ServerCertVerifier for SkipVerification {
    fn verify_server_cert(
        &self, _end_entity: &rustls::pki_types::CertificateDer,
        _intermediates: &[rustls::pki_types::CertificateDer],
        _server_name: &ServerName, _ocsp: &[u8], _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

pub struct Sender {
    send: SendStream,
}

pub struct Receiver {
    recv: RecvStream,
    buf: Vec<u8>,
}

pub struct Connection {
    pub sender: Sender,
    pub receiver: Receiver,
}

impl Connection {
    pub async fn connect(addr: &str) -> Result<Self> {
        let mut crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipVerification))
            .with_no_client_auth();
        crypto.alpn_protocols = vec![b"altreach".to_vec()];

        let client_config = ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?
        ));

        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(100_u32.into());
        transport.max_concurrent_uni_streams(100_u32.into());
        transport.keep_alive_interval(Some(Duration::from_secs(5)));

        let mut client_config = client_config;
        client_config.transport_config(Arc::new(transport));

        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);

        let conn = endpoint.connect(addr.parse()?, "altreach")?.await?;
        tracing::info!("QUIC connection established");

        let (send, recv) = conn.open_bi().await?;
        tracing::info!("Stream ready");

        Ok(Self {
            sender: Sender { send },
            receiver: Receiver { recv, buf: Vec::new() },
        })
    }
}

impl Sender {
    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let bytes = encode(msg)?;
        self.send.write_all(&bytes).await?;
        Ok(())
    }
}

impl Receiver {
    pub async fn recv(&mut self) -> Result<ServerMessage> {
        loop {
            if let Some((msg, consumed)) = decode::<ServerMessage>(&self.buf)? {
                self.buf.drain(..consumed);
                return Ok(msg);
            }

            let mut tmp = [0u8; 4096];
            match self.recv.read(&mut tmp).await? {
                Some(n) => self.buf.extend_from_slice(&tmp[..n]),
                None => anyhow::bail!("Stream closed"),
            }
        }
    }
}
