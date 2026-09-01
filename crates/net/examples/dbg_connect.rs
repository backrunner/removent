use removent_core::{DataPaths, identity};
use removent_net::{PinState, make_client_endpoint, make_server_endpoint};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let dir = tempfile::tempdir().unwrap();
    let p = DataPaths {
        root: dir.path().to_path_buf(),
    };
    let pc = DataPaths {
        root: p.root.join("c"),
    };
    let ph = DataPaths {
        root: p.root.join("h"),
    };
    let cid = identity::load_or_create(&pc, "C").unwrap();
    let hid = identity::load_or_create(&ph, "H").unwrap();

    let ep_server = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &hid,
        PinState::new([], true),
    )
    .unwrap();
    let addr = ep_server.0.local_addr().unwrap();
    eprintln!("server listening at {addr}");

    tokio::spawn(async move {
        match ep_server.0.accept().await {
            Some(inc) => match inc.await {
                Ok(_conn) => eprintln!("SERVER: connected"),
                Err(e) => eprintln!("SERVER: handshake failed: {e}"),
            },
            None => eprintln!("SERVER: closed"),
        }
    });

    let ep_client = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &cid,
        PinState::new([], true),
    )
    .unwrap();
    let connecting = ep_client.0.connect(addr, "removent").unwrap();
    match connecting.await {
        Ok(_) => eprintln!("CLIENT: connected"),
        Err(e) => eprintln!("CLIENT: failed: {e}"),
    }
}
