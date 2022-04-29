use std::time::Duration;

use ntp_proto::{
    NtpClock, NtpDuration, NtpHeader, Peer, PeerSnapshot, ReferenceId, SystemSnapshot,
};
use tokio::{
    net::{ToSocketAddrs, UdpSocket},
    sync::watch,
    time::Instant,
};

fn poll_interval_to_duration(poll_interval: i8) -> Duration {
    match poll_interval {
        i if i <= 0 => Duration::from_secs(1),
        i if i < 64 => Duration::from_secs(1 << i),
        _ => Duration::from_secs(std::u64::MAX),
    }
}

pub async fn start_peer<A: ToSocketAddrs, C: 'static + NtpClock + Send>(
    addr: A,
    clock: C,
    mut system_snapshots: watch::Receiver<SystemSnapshot>,
) -> Result<watch::Receiver<Option<PeerSnapshot>>, std::io::Error> {
    // setup socket
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(addr).await?;

    let (tx, rx) = watch::channel::<Option<PeerSnapshot>>(None);

    let our_id = ReferenceId::from_ip(socket.local_addr()?.ip());
    let peer_id = ReferenceId::from_ip(socket.peer_addr()?.ip());
    let socket = ntp_udp::UdpSocket::from_tokio(socket)?;

    tokio::spawn(async move {
        let mut peer = Peer::new(our_id, peer_id, clock.now().unwrap());

        let poll_interval = {
            let system_snapshot = system_snapshots.borrow_and_update();
            peer.get_interval_next_poll(system_snapshot.poll_interval)
        };
        let poll_wait = tokio::time::sleep(poll_interval_to_duration(poll_interval));
        tokio::pin!(poll_wait);

        loop {
            let mut buf = [0_u8; 48];

            tokio::select! {
                () = &mut poll_wait => {
                    let poll_interval = {
                        let system_snapshot = system_snapshots.borrow_and_update();
                        peer.get_interval_next_poll(system_snapshot.poll_interval)
                    };
                    poll_wait
                        .as_mut()
                        .reset(Instant::now() + poll_interval_to_duration(poll_interval));

                    // TODO: Figure out proper error behaviour here
                    let packet = peer.generate_poll_message(clock.now().unwrap());
                    socket.send(&packet.serialize()).await.unwrap();
                },
                result = socket.recv(&mut buf) => {
                    if let Ok((size, Some(timestamp))) = result {
                        // Note: packets are allowed to be bigger when including extensions.
                        // we don't expect them, but the server may still send them. The
                        // extra bytes are guaranteed safe to ignore. `recv` truncates the messages.
                        // Messages of fewer than 48 bytes are skipped entirely
                        if size < 48 {
                            // TODO log something
                        } else {
                            let system_snapshot = *system_snapshots.borrow_and_update();

                            let packet = NtpHeader::deserialize(&buf);
                            let result = peer.handle_incoming(packet, timestamp, system_snapshot.precision);

                            let system_poll = {
                                let system_snapshot = system_snapshots.borrow_and_update();
                                NtpDuration::from_exponent(system_snapshot.poll_interval)
                            };

                            if peer.accept_synchronization(timestamp, system_poll).is_err() {
                                let _ = tx.send(None);
                            } else if let Ok(update) = result {
                                let _ = tx.send(Some(update));
                            }
                        }
                    } else {
                        // TODO: log something
                    }
                },
            }
        }
    });

    Ok(rx)
}
