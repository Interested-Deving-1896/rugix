//! TCP idle read deadlines and cancellation beneath ureq's TLS transport.
//!
//! A body-phase deadline expires even while a large response makes progress.
//! This wrapper instead bounds each wait for input, preserving long streams.

use std::sync::Arc;
use std::time::Duration;

use ureq::unversioned::transport::time;
use ureq::unversioned::transport::Buffers;
use ureq::unversioned::transport::ConnectProxyConnector;
use ureq::unversioned::transport::ConnectionDetails;
use ureq::unversioned::transport::Connector;
use ureq::unversioned::transport::NextTimeout;
use ureq::unversioned::transport::RustlsConnector;
use ureq::unversioned::transport::TcpConnector;
use ureq::unversioned::transport::Transport;

use super::Cancellation;

/// Place cancellation below TLS, which can perform several socket reads before
/// returning plaintext. This chain preserves the enabled TCP, CONNECT, and Rustls
/// transports; SOCKS and native TLS are not enabled in this application.
pub(crate) fn connector(timeout: Duration, cancellation: Arc<Cancellation>) -> impl Connector<()> {
    ConnectProxyConnector::default()
        .chain(TcpConnector::default())
        .chain(IdleConnector {
            timeout,
            cancellation,
        })
        .chain(RustlsConnector::default())
}

#[derive(Debug)]
struct IdleConnector {
    pub timeout: Duration,
    pub cancellation: Arc<Cancellation>,
}

impl<T: Transport> Connector<T> for IdleConnector {
    type Out = IdleTransport<T>;

    fn connect(
        &self,
        _: &ConnectionDetails,
        transport: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(transport.map(|inner| IdleTransport {
            inner,
            timeout: self.timeout,
            cancellation: Arc::clone(&self.cancellation),
        }))
    }
}

#[derive(Debug)]
struct IdleTransport<T> {
    inner: T,
    timeout: Duration,
    cancellation: Arc<Cancellation>,
}

impl<T: Transport> Transport for IdleTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, ureq::Error> {
        if self.cancellation.is_canceled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "bundle download canceled",
            )
            .into());
        }
        if *timeout.after > self.timeout {
            timeout = NextTimeout {
                after: time::Duration::Exact(self.timeout),
                reason: ureq::Timeout::RecvBody,
            };
        }
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::process::Command;
    use std::process::Stdio;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use ureq::unversioned::resolver::DefaultResolver;

    use super::connector;
    use super::Cancellation;

    /// TLS control records cannot keep a canceled body reader inside Rustls indefinitely.
    #[test]
    fn cancellation_interrupts_tls_control_record_reads() {
        let directory = tempfile::tempdir().unwrap();
        let certificate_path = directory.path().join("certificate.pem");
        let key_path = directory.path().join("key.pem");
        assert!(Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=IP:127.0.0.1",
                "-addext",
                "basicConstraints=critical,CA:FALSE"
            ])
            .arg("-keyout")
            .arg(&key_path)
            .arg("-out")
            .arg(&certificate_path)
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let certificate =
            ureq::tls::Certificate::from_pem(&fs::read(certificate_path).unwrap()).unwrap();
        let key = Command::new("openssl")
            .args(["pkcs8", "-topk8", "-nocrypt", "-outform", "DER", "-in"])
            .arg(key_path)
            .output()
            .unwrap();
        assert!(key.status.success());
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    certificate.der().to_vec(),
                )],
                rustls::pki_types::PrivateKeyDer::try_from(key.stdout).unwrap(),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}/bundle", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let records = Arc::new(AtomicUsize::new(0));
        let server_stop = Arc::clone(&stop);
        let server_records = Arc::clone(&records);
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, socket);
            {
                let mut input = BufReader::new(&mut stream);
                loop {
                    let mut line = String::new();
                    input.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n0123")
                .unwrap();
            stream.flush().unwrap();
            for _ in 0..60 {
                if server_stop.load(Ordering::Relaxed) {
                    break;
                }
                stream.conn.refresh_traffic_keys().unwrap();
                if stream.conn.write_tls(&mut stream.sock).is_err() {
                    break;
                }
                server_records.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let cancellation = Arc::new(Cancellation::default());
        let config = ureq::Agent::config_builder()
            .timeout_send_request(Some(Duration::from_secs(2)))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs([certificate].into())
                    .unversioned_rustls_crypto_provider(provider)
                    .build(),
            )
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            connector(Duration::from_millis(300), Arc::clone(&cancellation)),
            DefaultResolver::default(),
        );
        let response = agent.get(url).call().unwrap();
        let mut reader = response.into_body().into_reader();
        reader.read_exact(&mut [0; 4]).unwrap();
        let (result, receiver) = std::sync::mpsc::channel();
        let client = std::thread::spawn(move || {
            let error = reader.read_exact(&mut [0; 4]);
            result.send(error).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        cancellation.cancel();
        let result = receiver.recv_timeout(Duration::from_secs(1));
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        client.join().unwrap();
        assert!(result.unwrap().is_err());
        assert!(records.load(Ordering::Relaxed) >= 2);
    }
}
