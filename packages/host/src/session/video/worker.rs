//! Own synchronous platform codec calls on a dedicated, bounded worker.
//! Never make a VideoToolbox flush or CPU conversion block Tokio's control,
//! audio, or network timers. Dropping the sender drains at most one queued job.

use removent_media_codec::VideoEncoder;
use removent_proto::CodecId;
use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut VideoEncoder) + Send>;

pub(super) struct EncoderWorker {
    jobs: mpsc::Sender<Job>,
}

impl EncoderWorker {
    pub async fn new(
        codec: CodecId,
        width: usize,
        height: usize,
        bitrate: u32,
        fps: u8,
    ) -> Result<Self, String> {
        let (jobs, mut receive) = mpsc::channel::<Job>(1);
        let (ready, started) = oneshot::channel();
        std::thread::Builder::new()
            .name("video-encoder".into())
            .spawn(move || {
                let mut encoder = match VideoEncoder::new(codec, width, height, bitrate, fps) {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while let Some(job) = receive.blocking_recv() {
                    job(&mut encoder);
                }
            })
            .map_err(|e| e.to_string())?;
        started
            .await
            .map_err(|_| "video encoder worker exited during startup".to_string())??;
        Ok(Self { jobs })
    }

    pub async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut VideoEncoder) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (reply, done) = oneshot::channel();
        self.jobs
            .send(Box::new(move |encoder| {
                let _ = reply.send(operation(encoder));
            }))
            .await
            .map_err(|_| "video encoder worker stopped".to_string())?;
        done.await
            .map_err(|_| "video encoder worker failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn codec_work_does_not_block_single_thread_runtime() {
        let worker = EncoderWorker::new(CodecId::H264, 64, 64, 1000, 60)
            .await
            .unwrap();
        let (entered, ready) = oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let job = worker.run(move |_| {
            let _ = entered.send(());
            gate.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        });
        let control = async {
            ready.await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(job, control);
        result.unwrap();
    }
}
