use std::net::SocketAddr;
use std::rc::Rc;

use store::key;
use store::value;

const OP_SIZE: usize = 1;
enum OP {
    Unknown,
    Get,
    Update,
}
const REQUEST_SIZE: usize = OP_SIZE + key::KEY_SIZE + value::RAW_VALUE_SIZE;
type Request = [u8; REQUEST_SIZE];
const RESPONSE_SIZE: usize = value::RAW_VALUE_SIZE;
type Response = [u8; RESPONSE_SIZE];
const EMPTY_RESPONSE: Response = value::EMPTY_RAW_VALUE;

impl OP {
    fn new(request: &Request) -> Self {
        match request[0] {
            1 => Self::Get,
            2 => Self::Update,
            _ => Self::Unknown,
        }
    }
}

fn main() {
    let store = Rc::new(store::XXH3KVStore::new());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build local runtime");
    let local = tokio::task::LocalSet::new();

    // Spawn maintenance tasks
    let store_2 = store.clone();
    local.spawn_local(async move {
        tokio::join! {
            store_2.stats_loop(),
            store_2.ttl_loop(),
        }
    });

    local.spawn_local(async move {
        // Create std::net::UdpSocket and bind it;
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        println!("UDP Server listening on {addr}");
        let error_msg = &format!("Failed to bind UDP socket on {addr}");
        let socket = net::udp_bind(addr).expect(error_msg);
        let socket = tokio::net::UdpSocket::from_std(socket).expect(error_msg);

        let mut request: Request = [0; REQUEST_SIZE];
        let mut response: Response = [0; RESPONSE_SIZE];

        loop {
            match socket.recv_from(&mut request).await {
                Ok((_, src_addr)) => {
                    match OP::new(&request) {
                        OP::Get => process_get_request(&store, &request, &mut response),
                        OP::Update => process_update_request(&store, &request, &mut response),
                        OP::Unknown => {
                            eprintln!("unable to understand operation: {:?}", request[0]);
                            response.copy_from_slice(&EMPTY_RESPONSE);
                        }
                    };
                    let _ = socket.send_to(&response, src_addr).await;
                }
                Err(e) => {
                    eprintln!("error receiving packet: {}", e)
                }
            }
        }
    });

    rt.block_on(local);
}

#[inline]
fn process_get_request(store: &store::XXH3KVStore, request: &Request, response: &mut Response) {
    let key = get_key(request);
    let val = store.get(key);
    response.copy_from_slice(&val);
}

#[inline]
fn process_update_request(store: &store::XXH3KVStore, request: &Request, response: &mut Response) {
    let key = get_key(request);
    let val = get_val(request);
    store.update(key, val);
    response.copy_from_slice(&val);
}

#[inline]
fn get_key(request: &Request) -> key::Key {
    request[1..1 + key::KEY_SIZE]
        .try_into()
        .unwrap_or_else(|e| {
            eprintln!(
                "failed to get key. error: {:?}. request.len: {:?}",
                e,
                request.len()
            );
            key::EMPTY_KEY
        })
}

#[inline]
fn get_val(request: &Request) -> value::RawValue {
    request[1 + key::KEY_SIZE..(1 + key::KEY_SIZE + value::RAW_VALUE_SIZE)]
        .try_into()
        .unwrap_or_else(|e| {
            eprintln!(
                "failed to get value. error: {:?}. request.len: {:?}",
                e,
                request.len()
            );
            value::EMPTY_RAW_VALUE
        })
}
