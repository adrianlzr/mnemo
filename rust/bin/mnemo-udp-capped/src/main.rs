use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

const STORE_CAP: usize = 100_000_000;
type Store = store_capped::xxh3_map!(STORE_CAP);
type Key = store_capped::Key;
type Value = store_capped::Value;

const KEY_SIZE: usize = store_capped::Key::SIZE;
const VALUE_SIZE: usize = store_capped::Value::SIZE;
const EMPTY_VALUE: [u8; 36] = [0u8; 36];

const OP_SIZE: usize = 1;
enum OP {
    Unknown,
    Get,
    Update,
}
const REQUEST_SIZE: usize = OP_SIZE + KEY_SIZE + VALUE_SIZE;
type Request = [u8; REQUEST_SIZE];
const RESPONSE_SIZE: usize = VALUE_SIZE;
type Response = [u8; RESPONSE_SIZE];

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
    time::init();
    let store: RefCell<&'static mut Store> = RefCell::new(Store::new());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build local runtime");
    let local = tokio::task::LocalSet::new();

    local.spawn_local(async move {
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        println!("UDP Server listening on {addr}");
        let error_msg = format!("Failed to bind UDP socket on {addr}");
        let socket = Rc::new(tokio::net::UdpSocket::bind(addr).await.expect(&error_msg));

        loop {
            let mut request: Request = [0; REQUEST_SIZE];
            match socket.recv_from(&mut request).await {
                Ok((_, src_addr)) => {
                    let now = time::Time::now();
                    let mut response: Response = [0; RESPONSE_SIZE];

                    match OP::new(&request) {
                        OP::Get => process_get_request(
                            &mut store.borrow_mut(),
                            &request,
                            &mut response,
                            now,
                        ),
                        OP::Update => process_update_request(
                            &mut store.borrow_mut(),
                            &request,
                            &mut response,
                            now,
                        ),
                        OP::Unknown => {
                            eprintln!("unable to understand operation: {:?}", request[0]);
                            response.copy_from_slice(&EMPTY_VALUE);
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
fn process_get_request(
    store: &mut Store,
    request: &Request,
    response: &mut Response,
    now: time::Time,
) {
    let key = get_key(request);
    if let Ok(val) = store.get(key, now) {
        response.copy_from_slice(&val.into_wire());
    };
}

#[inline]
fn process_update_request(
    store: &mut Store,
    request: &Request,
    response: &mut Response,
    now: time::Time,
) {
    let key = get_key(request);
    let val = get_val(request);
    if let Ok(updated_value) = store.insert(key, val, now, None) {
        response.copy_from_slice(&updated_value.into_wire());
    }
}

#[inline]
fn get_key(request: &Request) -> Key {
    request[1..1 + KEY_SIZE].into()
}

#[inline]
fn get_val(request: &Request) -> Value {
    let bytes: [u8; VALUE_SIZE] = request[1 + KEY_SIZE..(1 + KEY_SIZE + VALUE_SIZE)]
        .try_into()
        .expect("request slicing is infallible");
    bytes.into()
}
