//! The network steps below HTTP: name resolution and TCP connect.

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTPS_PORT: u16 = 443;

/// Splits a config `host` (`name`, `name:port`, `[v6]`, `[v6]:port` or a
/// bare v6 literal) into host and port; the port defaults to 443.
pub fn split_host_port(host: &str) -> (String, u16) {
    if let Some((addr, tail)) = host.strip_prefix('[').and_then(|r| r.split_once(']')) {
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(HTTPS_PORT);
        return (addr.to_string(), port);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') => match port.parse() {
            Ok(port) => (name.to_string(), port),
            Err(_) => (host.to_string(), HTTPS_PORT),
        },
        _ => (host.to_string(), HTTPS_PORT),
    }
}

pub fn resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no addresses"));
    }
    Ok(addrs)
}

/// The first of `addrs` that accepts a connection; the last error otherwise.
pub fn connect(addrs: &[SocketAddr]) -> io::Result<SocketAddr> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses");
    for addr in addrs {
        match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            Ok(_) => return Ok(*addr),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_host_port_forms() {
        let s = |h: &str| split_host_port(h);
        assert_eq!(s("192.168.1.1"), ("192.168.1.1".into(), 443));
        assert_eq!(s("nas.lan:8443"), ("nas.lan".into(), 8443));
        assert_eq!(s("[fd00::1]:8443"), ("fd00::1".into(), 8443));
        assert_eq!(s("[fd00::1]"), ("fd00::1".into(), 443));
        assert_eq!(s("fd00::1"), ("fd00::1".into(), 443));
        assert_eq!(s("nas.lan:abc"), ("nas.lan:abc".into(), 443));
    }

    #[test]
    fn resolve_ip_literals_and_failures() {
        assert_eq!(
            resolve("127.0.0.1", 8443).unwrap(),
            vec!["127.0.0.1:8443".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(resolve("::1", 443).unwrap()[0].port(), 443);
        assert!(resolve("synoctl-test.invalid", 443).is_err());
    }

    #[test]
    fn connect_finds_listener_and_reports_refusal() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap();
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        assert_eq!(connect(&[closed, open]).unwrap(), open);
        assert!(connect(&[closed]).is_err());
    }
}
