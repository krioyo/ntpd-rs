use crate::peer::{MsgForSystem, PeerIndex, PeerTask, ResetEpoch};
use ntp_os_clock::UnixNtpClock;
use ntp_proto::{
    ClockController, ClockUpdateResult, FilterAndCombine, NtpInstant, PeerSnapshot, SystemConfig,
    SystemSnapshot,
};
use tracing::info;

use std::{error::Error, sync::Arc};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy)]
enum PeerStatus {
    Demobilized,
    AwaitingReset,
    Valid(PeerSnapshot),
}

struct Peers {
    peers: Box<[PeerStatus]>,
}

impl Peers {
    fn new(length: usize) -> Self {
        Self {
            peers: vec![PeerStatus::AwaitingReset; length].into(),
        }
    }

    fn len(&self) -> usize {
        self.peers.len()
    }

    fn valid_snapshots(&self) -> impl Iterator<Item = PeerSnapshot> + '_ {
        self.peers
            .iter()
            .filter_map(|peer_status| match peer_status {
                PeerStatus::Demobilized | PeerStatus::AwaitingReset => None,
                PeerStatus::Valid(snapshot) => Some(*snapshot),
            })
    }

    fn receive_update(&mut self, msg: MsgForSystem, current_reset_epoch: ResetEpoch) {
        match msg {
            MsgForSystem::MustDemobilize(index) => {
                self.peers[index.index] = PeerStatus::Demobilized;
            }
            MsgForSystem::Snapshot(index, msg_reset_epoch, snapshot) => {
                if current_reset_epoch == msg_reset_epoch {
                    self.peers[index.index] = PeerStatus::Valid(snapshot);
                }
            }
        }
    }

    fn reset_all(&mut self) {
        for peer_status in self.peers.iter_mut() {
            *peer_status = match peer_status {
                PeerStatus::Demobilized => PeerStatus::Demobilized,
                PeerStatus::AwaitingReset => PeerStatus::AwaitingReset,
                PeerStatus::Valid(_) => PeerStatus::AwaitingReset,
            };
        }
    }
}

pub(crate) struct System {}

impl System {
    pub async fn spawn(
        config: &SystemConfig,
        peer_addresses: &[&str],
    ) -> Result<(), Box<dyn Error>> {
        // shares the system state with all peers
        let global_system_snapshot = Arc::new(tokio::sync::RwLock::new(SystemSnapshot::default()));

        // send the reset signal to all peers
        let reset_epoch: ResetEpoch = ResetEpoch::default();
        let (reset_tx, reset_rx) = watch::channel::<ResetEpoch>(reset_epoch);

        // receive peer snapshots from all peers
        let (msg_for_system_tx, msg_for_system_rx) = mpsc::channel::<MsgForSystem>(32);

        for (index, address) in peer_addresses.iter().enumerate() {
            PeerTask::spawn(
                PeerIndex { index },
                address,
                UnixNtpClock::new(),
                *config,
                msg_for_system_tx.clone(),
                global_system_snapshot.clone(),
                reset_rx.clone(),
            )
            .await?;
        }

        let mut peers = Peers::new(peer_addresses.len());

        Self::run_system(
            config,
            &mut peers,
            reset_epoch,
            global_system_snapshot,
            msg_for_system_rx,
            reset_tx,
        )
        .await
    }

    async fn run_system(
        config: &SystemConfig,
        peers: &mut Peers,
        mut reset_epoch: ResetEpoch,
        global_system_snapshot: Arc<tokio::sync::RwLock<SystemSnapshot>>,
        mut msg_for_system_rx: mpsc::Receiver<MsgForSystem>,
        reset_tx: watch::Sender<ResetEpoch>,
    ) -> Result<(), Box<dyn Error>> {
        let mut controller = ClockController::new(UnixNtpClock::new());
        let mut snapshots = Vec::with_capacity(peers.len());

        while let Some(msg_for_system) = msg_for_system_rx.recv().await {
            peers.receive_update(msg_for_system, reset_epoch);

            // remove snapshots from previous iteration
            snapshots.clear();

            // add all valid measurements to our list of snapshots
            snapshots.extend(peers.valid_snapshots());

            let ntp_instant = NtpInstant::now();
            let system_poll = global_system_snapshot.read().await.poll_interval;
            let result = FilterAndCombine::run(config, &snapshots, ntp_instant, system_poll);

            let clock_select = match result {
                Some(clock_select) => clock_select,
                None => {
                    info!("filter and combine did not produce a result");
                    continue;
                }
            };

            let offset_ms = clock_select.system_offset.to_seconds() * 1000.0;
            let jitter_ms = clock_select.system_jitter.to_seconds() * 1000.0;
            info!(offset_ms, jitter_ms, "system offset and jitter");

            let adjust_type = controller.update(
                clock_select.system_offset,
                clock_select.system_jitter,
                clock_select.system_peer_snapshot.root_delay,
                clock_select.system_peer_snapshot.root_dispersion,
                clock_select.system_peer_snapshot.leap_indicator,
                clock_select.system_peer_snapshot.time,
            );

            // Handle situations needing extra processing
            match adjust_type {
                ClockUpdateResult::Panic => {
                    panic!(
                        r"Unusually large clock step suggested,
                            please manually verify system clock and reference clock 
                                 state and restart if appropriate."
                    )
                }
                ClockUpdateResult::Step => {
                    peers.reset_all();

                    reset_epoch = reset_epoch.inc();
                    reset_tx.send_replace(reset_epoch);
                }
                _ => {}
            }

            // Handle updating system snapshot
            if let ClockUpdateResult::Ignore = adjust_type {
                // ignore this update
            } else {
                let mut global = global_system_snapshot.write().await;
                global.poll_interval = controller.preferred_poll_interval();
                global.leap_indicator = clock_select.system_peer_snapshot.leap_indicator;
            }
        }

        // the channel closed and has no more messages in it
        Ok(())
    }
}
