use socket2::{Domain, Protocol, Socket, Type};
use std::net::{SocketAddr, TcpListener, UdpSocket};

#[allow(dead_code)]
pub fn udp_bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };

    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?; // this is the SO_REUSEPORT bit

    socket.bind(&addr.into())?;

    // socket2::Socket -> std::net::UdpSocket -> tokio::net::UdpSocket
    Ok(socket.into())
}

#[allow(dead_code)]
pub fn tcp_bind(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
    let sock = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    sock.set_reuse_address(true)?;
    sock.set_reuse_port(true)?; // MUST come before bind
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    Ok(sock.into())
}
