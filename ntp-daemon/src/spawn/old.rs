#[derive(Debug)]
struct Spawner {
    pools: HashMap<PoolIndex, Arc<tokio::sync::Mutex<PoolAddresses>>>,
    sender: Sender<SpawnTask>,
}

impl Spawner {
    async fn spawn(&mut self, config: SpawnConfig) -> tokio::task::JoinHandle<()> {
        let sender = self.sender.clone();

        match config {
            SpawnConfig::Standard { config } => tokio::spawn(Self::spawn_standard(config, sender)),

            SpawnConfig::Pool {
                config,
                index,
                in_use,
            } => {
                let pool = self.pools.entry(index).or_default().clone();
                tokio::spawn(Self::spawn_pool(index, pool, config, in_use, sender))
            }
        }
    }

    async fn spawn_standard(config: StandardPeerConfig, sender: Sender<SpawnTask>) {
        let addr = loop {
            match config.addr.lookup_host().await {
                Ok(mut addresses) => match addresses.next() {
                    None => {
                        warn!("Could not resolve peer address, retrying");
                        tokio::time::sleep(NETWORK_WAIT_PERIOD).await
                    }
                    Some(first) => {
                        break first;
                    }
                },
                Err(e) => {
                    warn!(error = ?e, "error while resolving peer address, retrying");
                    tokio::time::sleep(NETWORK_WAIT_PERIOD).await
                }
            }
        };

        let spawn_task = SpawnTask {
            peer_address: PeerAddress::Peer {
                address: config.addr,
            },
            address: addr,
        };

        if let Err(send_error) = sender.send(spawn_task).await {
            tracing::error!(?send_error, "Receive half got disconnected");
        }
    }

    async fn spawn_pool(
        pool_index: PoolIndex,
        pool: Arc<tokio::sync::Mutex<PoolAddresses>>,
        config: PoolPeerConfig,
        in_use: Vec<SocketAddr>,
        sender: Sender<SpawnTask>,
    ) {
        let mut wait_period = NETWORK_WAIT_PERIOD;
        let mut remaining;

        loop {
            let mut pool = pool.lock().await;

            remaining = config.max_peers - in_use.len();

            if pool.backups.len() < config.max_peers - in_use.len() {
                match config.addr.lookup_host().await {
                    Ok(addresses) => {
                        pool.backups = addresses.collect();
                    }
                    Err(e) => {
                        warn!(error = ?e, "error while resolving peer address, retrying");
                        tokio::time::sleep(wait_period).await;
                        continue;
                    }
                }
            }

            // then, empty out our backups
            while let Some(addr) = pool.backups.pop() {
                if remaining == 0 {
                    return;
                }

                debug_assert!(!in_use.contains(&addr));

                let spawn_task = SpawnTask {
                    peer_address: PeerAddress::Pool {
                        index: pool_index,
                        address: config.addr.clone(),
                        socket_address: addr,
                        max_peers: config.max_peers,
                    },
                    address: addr,
                };

                tracing::debug!(?spawn_task, "intending to spawn new pool peer at");

                if let Err(send_error) = sender.send(spawn_task).await {
                    tracing::error!(?send_error, "Receive half got disconnected");
                }

                remaining -= 1;
            }

            if remaining == 0 {
                return;
            }

            let wait_period_max = if cfg!(test) {
                std::time::Duration::default()
            } else {
                std::time::Duration::from_secs(60)
            };

            wait_period = Ord::min(2 * wait_period, wait_period_max);

            warn!(?pool_index, remaining, "could not fully fill pool");
            tokio::time::sleep(wait_period).await;
        }
    }
}
