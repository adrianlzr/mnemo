use time::Time;
// use time::Time;
use clap::Parser;
use udp_server::{Packet, UdpServer};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 1)]
    io_threads: usize,
}

const INPUT_SIZE: usize = 45;
const OUTPUT_SIZE: usize = 36;

type InputPacket = Packet<INPUT_SIZE>;
type OutputPacket = Packet<OUTPUT_SIZE>;

const STORE_CAP: usize = 100_000_000;
type Store = store_capped::xxh3_map!(STORE_CAP);
type Key = store_capped::Key;
type Value = store_capped::Value;

enum OP {
    Unknown,
    Get,
    Update,
}

impl From<&InputPacket> for OP {
    fn from(packet: &InputPacket) -> Self {
        match packet.data[0] {
            1 => Self::Get,
            2 => Self::Update,
            _ => Self::Unknown,
        }
    }
}

fn main() {
    let args = Args::parse();
    let store: &'static mut Store = Store::new();

    let server = UdpServer::new(
        "0.0.0.0".into(),
        8080,
        args.io_threads,
        move |packet: InputPacket| -> OutputPacket {
            let op = OP::from(&packet);
            let mut response = [0u8; OUTPUT_SIZE];

            match op {
                OP::Unknown => (),
                OP::Get => process_get_request(store, &packet, &mut response),
                OP::Update => process_update_request(store, &packet, &mut response),
            };

            OutputPacket {
                data: response,
                peer: packet.peer,
            }
        },
    );

    server.run();
}

#[inline]
fn process_get_request(store: &mut Store, packet: &InputPacket, response: &mut [u8; 36]) {
    let key = get_key(packet);

    if let Ok(val) = store.get(key, Time::now()) {
        response.copy_from_slice(&val.into_wire());
    }
}

#[inline]
fn process_update_request(store: &mut Store, packet: &InputPacket, response: &mut [u8; 36]) {
    let key = get_key(packet);
    let val = get_val(packet);

    if let Ok(updated_val) = store.insert(key, val, Time::now(), None) {
        response.copy_from_slice(&updated_val.into_wire());
    }
}

#[inline]
fn get_key(packet: &InputPacket) -> Key {
    Key::from_wire(&packet.data[1..9])
}

#[inline]
fn get_val(packet: &InputPacket) -> Value {
    Value::from_wire(&packet.data[1 + 8..])
}
