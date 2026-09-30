//! Local, unprivileged impairment. No changes to system routes or interfaces.
use anyhow::Result;
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::Instant,
};

#[derive(Clone, Copy)]
pub struct Link {
    pub delay_ms: u64,
    pub kbps: u64,
    pub loss_percent: u32,
    pub stall_ms: u64,
}
pub struct Proxy {
    pub address: SocketAddr,
    pub drops: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Pacer {
    finish: Instant,
    stall_at: Instant,
    random: u32,
}
impl Pacer {
    fn new(seed: u32) -> Self {
        Self {
            finish: Instant::now(),
            stall_at: Instant::now() + Duration::from_secs(1),
            random: seed,
        }
    }
    fn due(&mut self, bytes: usize, link: Link) -> Instant {
        self.finish = self.finish.max(Instant::now())
            + Duration::from_secs_f64(bytes as f64 * 8. / (link.kbps * 1000) as f64);
        if link.stall_ms > 0 && self.finish >= self.stall_at {
            self.finish += Duration::from_millis(link.stall_ms);
            self.stall_at = self.finish + Duration::from_secs(1);
        }
        self.finish + Duration::from_millis(link.delay_ms)
    }
    fn lost(&mut self, percent: u32) -> bool {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random % 10_000 < percent * 100
    }
}

impl Proxy {
    pub async fn udp(target: SocketAddr, link: Link, seed: u32) -> Result<Self> {
        let front = UdpSocket::bind("127.0.0.1:0").await?;
        let back = UdpSocket::bind("127.0.0.1:0").await?;
        back.connect(target).await?;
        let address = front.local_addr()?;
        let drops = Arc::new(AtomicU64::new(0));
        let dropped = drops.clone();
        let task = tokio::spawn(async move {
            let mut peer = None;
            let mut a = [0; 65536];
            let mut b = [0; 65536];
            let mut queues: [VecDeque<(Instant, Vec<u8>)>; 2] = Default::default();
            let mut clocks = [Pacer::new(seed), Pacer::new(seed ^ 0x91af)];
            loop {
                let next = queues
                    .iter()
                    .enumerate()
                    .filter_map(|(i, q)| q.front().map(|p| (i, p.0)))
                    .min_by_key(|p| p.1);
                let deadline = next
                    .map(|p| p.1)
                    .unwrap_or(Instant::now() + Duration::from_secs(3600));
                let packet = tokio::select! {
                    result = front.recv_from(&mut a) => {
                        let Ok((n, addr)) = result else { break; };
                        if peer.is_some_and(|p| p != addr) { continue; }
                        peer = Some(addr);
                        Some((0, a[..n].to_vec()))
                    }
                    result = back.recv(&mut b) => {
                        let Ok(n) = result else { break; };
                        Some((1, b[..n].to_vec()))
                    }
                    _ = tokio::time::sleep_until(deadline), if next.is_some() => {
                        let i = next.unwrap().0;
                        let (_, bytes) = queues[i].pop_front().unwrap();
                        if i == 0 { let _ = back.send(&bytes).await; }
                        else if let Some(peer) = peer { let _ = front.send_to(&bytes,peer).await; }
                        None
                    }
                };
                if let Some((i, bytes)) = packet {
                    // 50 ms tail-drop router queue, independently in each direction.
                    // Random loss is in addition to capacity-induced drops.
                    if queues[i].len() >= 4096
                        || clocks[i].finish > Instant::now() + Duration::from_millis(50)
                    {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        let due = clocks[i].due(bytes.len(), link);
                        // Random loss occurs after serialization and consumes
                        // bandwidth; queue-overflow drops above do not.
                        if clocks[i].lost(link.loss_percent) {
                            dropped.fetch_add(1, Ordering::Relaxed);
                        } else {
                            queues[i].push_back((due, bytes));
                        }
                    }
                }
            }
        });
        Ok(Self {
            address,
            drops,
            task,
        })
    }

    pub async fn tcp(target: SocketAddr, link: Link) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((front,_)) = accepted else { break; };
                        tasks.spawn(async move {
                            let back = TcpStream::connect(target).await?;
                            front.set_nodelay(true)?; back.set_nodelay(true)?;
                            let (fr,fw) = front.into_split(); let (br,bw) = back.into_split();
                            tokio::try_join!(tcp_direction(fr,bw,link),tcp_direction(br,fw,link))?;
                            Ok::<_, std::io::Error>(())
                        });
                    }
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                }
            }
        });
        Ok(Self {
            address,
            drops: Arc::new(AtomicU64::new(0)),
            task,
        })
    }
}

async fn tcp_direction(
    mut read: tokio::net::tcp::OwnedReadHalf,
    mut write: tokio::net::tcp::OwnedWriteHalf,
    link: Link,
) -> std::io::Result<()> {
    // Ordered byte-stream stalls model TCP recovery/HOL. They are deliberately
    // NOT described as actual packet loss: dropping bytes would corrupt TCP.
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let produce = async {
        let mut pacer = Pacer::new(1);
        let mut bytes = [0; 4096];
        loop {
            let n = read.read(&mut bytes).await?;
            if n == 0 {
                break;
            }
            let due = pacer.due(n, link);
            if tx.send((due, bytes[..n].to_vec())).await.is_err() {
                break;
            }
        }
        drop(tx);
        Ok::<_, std::io::Error>(())
    };
    let consume = async {
        while let Some((due, bytes)) = rx.recv().await {
            tokio::time::sleep_until(due).await;
            write.write_all(&bytes).await?;
        }
        write.shutdown().await
    };
    tokio::try_join!(produce, consume)?;
    Ok(())
}
