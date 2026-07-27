use std::{net::UdpSocket, os::unix::net::SocketAddr};

pub struct UdpPacket<const SIZE: usize> {
    data: [u8; SIZE],
    peer: SocketAddr,
}

pub struct UdpServer<const INPUT_SIZE: usize, const OUTPUT_SIZE: usize, PacketHandler>
where
    PacketHandler: FnMut(UdpPacket<INPUT_SIZE>) -> UdpPacket<OUTPUT_SIZE>,
{
    socket: UdpSocket,
    handler: PacketHandler,
}

impl<const INPUT_SIZE: usize, const OUTPUT_SIZE: usize, PacketHandler>
    UdpServer<INPUT_SIZE, OUTPUT_SIZE, PacketHandler>
where
    PacketHandler: FnMut(UdpPacket<INPUT_SIZE>) -> UdpPacket<OUTPUT_SIZE>,
{
    pub fn new() {}
}
