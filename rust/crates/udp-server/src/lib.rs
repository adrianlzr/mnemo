use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;

use flume::{unbounded, Receiver, Sender};

// const CAP: usize = 100000;

#[derive(Debug)]
pub struct Packet<const SIZE: usize> {
    pub data: [u8; SIZE],
    pub peer: SocketAddr,
}

impl<const SIZE: usize> Packet<SIZE> {
    pub fn new(data: [u8; SIZE], peer: SocketAddr) -> Self {
        Self { data, peer }
    }
}

pub struct UdpServer<const INPUT_SIZE: usize, const OUTPUT_SIZE: usize, PacketHandler>
where
    PacketHandler: FnMut(Packet<INPUT_SIZE>) -> Packet<OUTPUT_SIZE> + Send + 'static,
{
    io_threads: usize,
    handler: PacketHandler,
    addr: SocketAddr, // socket: Arc<UdpSocket>,
}

impl<const INPUT_SIZE: usize, const OUTPUT_SIZE: usize, PacketHandler>
    UdpServer<INPUT_SIZE, OUTPUT_SIZE, PacketHandler>
where
    PacketHandler: FnMut(Packet<INPUT_SIZE>) -> Packet<OUTPUT_SIZE> + Send + 'static,
{
    pub fn new(ip: String, port: u16, io_threads: usize, handler: PacketHandler) -> Self {
        let ip = IpAddr::V4(ip.parse().unwrap());
        let addr = SocketAddr::new(ip, port);
        // let socket = Arc::new(net::udp_bind(addr).expect("failed to bind"));
        Self {
            io_threads,
            handler,
            addr,
            // socket,
        }
    }

    fn io_reader(socket: Arc<UdpSocket>, tx: Sender<Packet<INPUT_SIZE>>) {
        let mut buffer = [0u8; INPUT_SIZE];
        loop {
            match socket.recv_from(&mut buffer) {
                Ok((_, peer)) => {
                    let packet = Packet::new(buffer, peer);
                    tx.send(packet).expect("Failed to send");
                }
                Err(e) => {
                    eprintln!("Encountered error: {}", e)
                }
            }
        }
    }

    fn io_handler(
        mut handler: PacketHandler,
        irx: Receiver<Packet<INPUT_SIZE>>,
        otx: Sender<Packet<OUTPUT_SIZE>>,
    ) {
        loop {
            match irx.recv() {
                Ok(packet) => {
                    let packet = (handler)(packet);

                    let _ = otx.send(packet);
                }
                Err(e) => eprintln!("Recieve error: {e}"),
            }
        }
    }

    fn io_writer(socket: Arc<UdpSocket>, rx: Receiver<Packet<OUTPUT_SIZE>>) {
        loop {
            match rx.recv() {
                Ok(packet) => {
                    if let Err(e) = socket.send_to(&packet.data, packet.peer) {
                        eprintln!("Failed to write: {e}")
                    }
                }
                Err(e) => eprintln!("Failed to receive from main worker thread: {}", e),
            }
        }
    }

    pub fn run(self) {
        let (itx, irx) = unbounded::<Packet<INPUT_SIZE>>();
        let (otx, orx) = unbounded::<Packet<OUTPUT_SIZE>>();

        std::thread::spawn(move || {
            Self::io_handler(self.handler, irx, otx);
        });

        for _ in 0..self.io_threads {
            // One bind per worker — this socket is the only reuseport-group
            // member this worker contributes.
            let socket = Arc::new(net::udp_bind(self.addr).expect("failed to bind"));

            let reader_socket = Arc::clone(&socket);
            let itx = itx.clone();
            std::thread::spawn(move || {
                Self::io_reader(reader_socket, itx);
            });

            let writer_socket = socket; // move the last clone in, no need for another Arc::clone
            let orx = orx.clone();
            std::thread::spawn(move || {
                Self::io_writer(writer_socket, orx);
            });
        }

        std::thread::park();
    }
}
