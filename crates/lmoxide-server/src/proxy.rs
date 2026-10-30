//! The PROXY protocol (HAProxy's, versions 1 and 2) on the game ports, for a TCP proxy in front
//! of the server (Traefik, HAProxy, nginx stream). Without it every player would have the
//! proxy's address, and the per-address login lockout and connection cap would hit them all at
//! once. Only connections from `--proxy-protocol-from` are read for a header, and they must send
//! one; others are taken as they are (docs/HOSTING.md "Behind a TCP proxy").

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, bail};
use ipnet::IpNet;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// How long a proxy may take to send its header.
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
/// Version 2's signature.
const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// Longest version 1 header, CRLF included.
const V1_MAX: usize = 107;
/// Longest version 2 address block read (TLVs are skipped, not used).
const V2_MAX: usize = 2048;

/// A proxy address or network, e.g. `172.18.0.0/16` or `10.0.0.5`.
pub fn parse_trusted(s: &str) -> Result<IpNet, String> {
    let s = s.trim();
    s.parse::<IpNet>().or_else(|_| s.parse::<IpAddr>().map(IpNet::from)).map_err(|e| format!("{s:?}: {e}"))
}

/// The client's address: from the PROXY header when `peer` is a trusted proxy, else `peer`.
pub async fn client_addr(sock: &mut TcpStream, peer: SocketAddr, trusted: &[IpNet]) -> anyhow::Result<SocketAddr> {
    if !trusted.iter().any(|n| n.contains(&canonical(peer.ip()))) {
        return Ok(peer);
    }
    let addr = tokio::time::timeout(HEADER_TIMEOUT, read_header(sock)).await.context("no PROXY header in time")??;
    // LOCAL (the proxy's own health checks) and unknown families: the proxy itself
    Ok(addr.unwrap_or(peer))
}

/// An IPv4 address mapped into IPv6 (dual-stack sockets) as plain IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

async fn read_header(sock: &mut TcpStream) -> anyhow::Result<Option<SocketAddr>> {
    let first = sock.read_u8().await?;
    match first {
        b'P' => {
            // version 1, one byte at a time so nothing after the header is consumed
            let mut line = vec![first];
            while !line.ends_with(b"\r\n") {
                if line.len() >= V1_MAX {
                    bail!("PROXY v1 header too long");
                }
                line.push(sock.read_u8().await?);
            }
            parse_v1(&line)
        }
        b'\r' => {
            let mut head = [0u8; 16];
            head[0] = first;
            sock.read_exact(&mut head[1..]).await?;
            let len = u16::from_be_bytes([head[14], head[15]]) as usize;
            if len > V2_MAX {
                bail!("PROXY v2 header too long ({len} bytes)");
            }
            let mut rest = vec![0u8; len];
            sock.read_exact(&mut rest).await?;
            parse_v2(&head, &rest)
        }
        _ => bail!("expected a PROXY header from the proxy"),
    }
}

/// `PROXY TCP4 <src> <dst> <sport> <dport>\r\n` (or `PROXY UNKNOWN ...`).
fn parse_v1(line: &[u8]) -> anyhow::Result<Option<SocketAddr>> {
    let line = std::str::from_utf8(line).context("PROXY v1 header isn't text")?.trim_end_matches("\r\n");
    let parts: Vec<&str> = line.split(' ').collect();
    match parts.as_slice() {
        ["PROXY", "UNKNOWN", ..] => Ok(None),
        ["PROXY", "TCP4" | "TCP6", src, _dst, sport, _dport] => {
            let ip: IpAddr = src.parse().context("PROXY v1 source address")?;
            let port: u16 = sport.parse().context("PROXY v1 source port")?;
            Ok(Some(SocketAddr::new(canonical(ip), port)))
        }
        _ => bail!("bad PROXY v1 header {line:?}"),
    }
}

/// Version 2: the 16-byte head (signature, version/command, family, length) and the rest.
fn parse_v2(head: &[u8; 16], rest: &[u8]) -> anyhow::Result<Option<SocketAddr>> {
    if head[..12] != V2_SIGNATURE {
        bail!("bad PROXY v2 signature");
    }
    if head[12] >> 4 != 2 {
        bail!("PROXY version {} isn't supported", head[12] >> 4);
    }
    match head[12] & 0x0f {
        0 => return Ok(None), // LOCAL
        1 => {}               // PROXY
        c => bail!("PROXY v2 command {c}"),
    }
    match head[13] >> 4 {
        // AF_INET: src, dst, sport, dport
        1 if rest.len() >= 12 => {
            let ip = Ipv4Addr::new(rest[0], rest[1], rest[2], rest[3]);
            Ok(Some(SocketAddr::new(IpAddr::V4(ip), u16::from_be_bytes([rest[8], rest[9]]))))
        }
        // AF_INET6
        2 if rest.len() >= 36 => {
            let ip = Ipv6Addr::from(<[u8; 16]>::try_from(&rest[..16]).unwrap());
            Ok(Some(SocketAddr::new(canonical(IpAddr::V6(ip)), u16::from_be_bytes([rest[32], rest[33]]))))
        }
        1 | 2 => bail!("PROXY v2 address block too short"),
        // AF_UNSPEC, AF_UNIX: no client address
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn v1() {
        assert_eq!(parse_v1(b"PROXY TCP4 203.0.113.7 10.0.0.2 51234 7100\r\n").unwrap(), Some("203.0.113.7:51234".parse().unwrap()));
        assert_eq!(parse_v1(b"PROXY TCP6 2001:db8::1 2001:db8::2 4000 7100\r\n").unwrap(), Some("[2001:db8::1]:4000".parse().unwrap()));
        assert_eq!(parse_v1(b"PROXY UNKNOWN\r\n").unwrap(), None);
        assert!(parse_v1(b"PROXY TCP4 nonsense\r\n").is_err());
    }

    #[test]
    fn v2() {
        let mut head = [0u8; 16];
        head[..12].copy_from_slice(&V2_SIGNATURE);
        head[12] = 0x21; // version 2, PROXY
        head[13] = 0x11; // AF_INET, STREAM
        let rest = [203, 0, 113, 7, 10, 0, 0, 2, 0xc8, 0x22, 0x1b, 0xbc];
        assert_eq!(parse_v2(&head, &rest).unwrap(), Some("203.0.113.7:51234".parse().unwrap()));
        head[12] = 0x20; // LOCAL
        assert_eq!(parse_v2(&head, &rest).unwrap(), None);
        head[12] = 0x11; // version 1 in a v2 frame
        assert!(parse_v2(&head, &rest).is_err());
    }

    #[test]
    fn trusted() {
        let nets = vec![parse_trusted("172.18.0.0/16").unwrap(), parse_trusted("10.0.0.5").unwrap()];
        assert!(nets.iter().any(|n| n.contains(&canonical("::ffff:172.18.3.4".parse().unwrap()))));
        assert!(nets.iter().any(|n| n.contains(&"10.0.0.5".parse::<IpAddr>().unwrap())));
        assert!(!nets.iter().any(|n| n.contains(&"10.0.0.6".parse::<IpAddr>().unwrap())));
        assert!(parse_trusted("nope").is_err());
    }

    /// A header sent by a trusted proxy replaces the address and leaves what follows unread.
    #[tokio::test]
    async fn reads_only_the_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(b"PROXY TCP4 198.51.100.9 127.0.0.1 40000 7100\r\nhello").await.unwrap();
            s
        });
        let (mut sock, peer) = listener.accept().await.unwrap();
        let trusted = vec![parse_trusted("127.0.0.1").unwrap()];
        assert_eq!(client_addr(&mut sock, peer, &trusted).await.unwrap(), "198.51.100.9:40000".parse().unwrap());
        let mut rest = [0u8; 5];
        sock.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"hello");
        // an untrusted peer is taken as it is, its bytes untouched
        assert_eq!(client_addr(&mut sock, peer, &[]).await.unwrap(), peer);
        drop(client.await.unwrap());
    }
}
