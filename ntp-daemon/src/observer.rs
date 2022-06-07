use crate::Peers;
use ntp_proto::SystemSnapshot;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub enum Observe {
    Peers,
    System,
}

pub async fn spawn(
    config: &crate::config::ObserveConfig,
    peers_reader: Arc<tokio::sync::RwLock<Peers>>,
    system_reader: Arc<tokio::sync::RwLock<SystemSnapshot>>,
) -> JoinHandle<std::io::Result<()>> {
    tokio::spawn(peer_state_observer(
        config.clone(),
        peers_reader,
        system_reader,
    ))
}

async fn peer_state_observer(
    config: crate::config::ObserveConfig,
    peers_reader: Arc<tokio::sync::RwLock<Peers>>,
    system_reader: Arc<tokio::sync::RwLock<SystemSnapshot>>,
) -> std::io::Result<()> {
    let socket_directory = &config.path;

    // create the path if it does not exist
    std::fs::create_dir_all(socket_directory)?;

    let observe_socket_path = socket_directory.join("observe");
    std::fs::remove_file(&observe_socket_path)?;
    let peers_listener = UnixListener::bind(&observe_socket_path)?;

    // this binary needs to run as root to be able to adjust the system clock.
    // by default, the socket inherits root permissions, but the client should not need
    // elevated permissions to read from the socket. So we explicitly set the permissions
    let permissions: std::fs::Permissions = PermissionsExt::from_mode(config.mode);
    std::fs::set_permissions(&observe_socket_path, permissions)?;

    let mut observed = Vec::with_capacity(8);
    let mut msg = Vec::with_capacity(16 * 1024);

    loop {
        let (mut stream, _addr) = peers_listener.accept().await?;

        let operation: Observe = crate::sockets::read_json(&mut stream, &mut msg).await?;

        match operation {
            Observe::Peers => {
                {
                    let state = peers_reader.read().await;

                    observed.clear();
                    observed.extend(state.observe());
                }

                crate::sockets::write_json(&mut stream, &observed).await?;
            }
            Observe::System => {
                let state = *system_reader.read().await;
                crate::sockets::write_json(&mut stream, &state).await?;
            }
        }
    }
}
