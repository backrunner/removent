//! Same-user bridge to an already established controller session.
//! Each attachment is pinned to a random session generation; it never silently follows reconnects.
use super::*;
use removent_net::RvpConnection;
use removent_proto::automation::{
    Failure, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Request, Response,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Info {
    pub generation: String,
    pub fingerprint: String,
    pub features: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    generation: Option<String>,
    request: Option<Request>,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    info: Info,
    response: Option<Response>,
}

pub struct Publisher {
    task: tokio::task::JoinHandle<()>,
    path: PathBuf,
    _lock: removent_core::DataDirLock,
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}
fn path(paths: &DataPaths, host: [u8; 32]) -> PathBuf {
    paths
        .run_dir()
        .join(format!("cu-{}.sock", hex::encode(&host[..8])))
}
fn same_user(stream: &UnixStream) -> bool {
    stream
        .peer_cred()
        .is_ok_and(|cred| cred.uid() == unsafe { libc::geteuid() })
}

impl Publisher {
    pub fn start(paths: &DataPaths, conn: RvpConnection, features: u64) -> anyhow::Result<Self> {
        paths.ensure_layout()?;
        let host = conn.peer_fingerprint().context("Peer identity missing")?;
        let path = path(paths, host);
        let lock = removent_core::DataDirLock::acquire_at(&path.with_extension("lock"))?;
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            ensure!(
                meta.file_type().is_socket(),
                "Unexpected automation socket path"
            );
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let info = Info {
            generation: hex::encode(rand::random::<[u8; 16]>()),
            fingerprint: hex::encode(host),
            features,
        };
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let lease = Arc::new(tokio::sync::Semaphore::new(1));
            loop {
                tokio::select! {biased;
                    _=conn.inner().closed()=>break,
                    Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
                    accepted=listener.accept(),if tasks.len()<8=>{
                        let Ok((mut socket,_))=accepted else{break};if !same_user(&socket){continue}
                        let info=info.clone();let conn=conn.clone();let lease=lease.clone();
                        tasks.spawn(async move{
                            let work=async{
                                let call:Call=read(&mut socket,MAX_REQUEST_BYTES+1024).await?;
                                let response=if let Some(request)=call.request {
                                    let error=if call.generation.as_deref()!=Some(&info.generation){Some(Failure::new("session_closed","Desktop connection changed; explicitly reconnect MCP"))}else{None};
                                    let permit=lease.try_acquire_owned();
                                    if let Some(error)=error{Some(Response{version:1,operation_id:request.operation_id,result:Err(error)})}
                                    else if permit.is_ok() || matches!(request.command, removent_proto::automation::Command::ControlStatus {}) {
                                        let _permit = permit.ok();
                                        let mut byte=[0];
                                        Some(tokio::select!{biased;
                                            _=socket.read(&mut byte)=>return Ok::<(),anyhow::Error>(()),
                                            response=removent_net::automation::call(&conn,&request)=>response?,
                                        })
                                    }else{Some(Response{version:1,operation_id:request.operation_id,result:Err(Failure::new("busy","Another local controller is using this session"))})}
                                }else{None};
                                write(&mut socket,&Reply{info,response},MAX_RESPONSE_BYTES+1024).await
                            };
                            let _=tokio::time::timeout(Duration::from_secs(35),work).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            task,
            path,
            _lock: lock,
        })
    }
}
#[derive(Clone)]
pub struct Attachment {
    path: PathBuf,
    pub info: Info,
}
impl Attachment {
    pub async fn find(paths: &DataPaths, host: [u8; 32]) -> anyhow::Result<Option<Self>> {
        let path = path(paths, host);
        let mut socket = match UnixStream::connect(&path).await {
            Ok(s) => s,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        ensure!(
            same_user(&socket),
            "Automation broker belongs to another user"
        );
        let reply = tokio::time::timeout(Duration::from_secs(2), async {
            write(
                &mut socket,
                &Call {
                    generation: None,
                    request: None,
                },
                1024,
            )
            .await?;
            read::<Reply>(&mut socket, 4096).await
        })
        .await??;
        ensure!(
            reply.info.fingerprint == hex::encode(host),
            "Automation broker identity mismatch"
        );
        Ok(Some(Self {
            path,
            info: reply.info,
        }))
    }
    pub async fn alive(&self) -> bool {
        let probe = async {
            let mut socket = UnixStream::connect(&self.path).await?;
            ensure!(same_user(&socket), "Automation broker owner mismatch");
            write(
                &mut socket,
                &Call {
                    generation: None,
                    request: None,
                },
                1024,
            )
            .await?;
            let reply: Reply = read(&mut socket, 4096).await?;
            Ok::<bool, anyhow::Error>(
                reply.info.generation == self.info.generation
                    && reply.info.fingerprint == self.info.fingerprint,
            )
        };
        matches!(
            tokio::time::timeout(Duration::from_secs(2), probe).await,
            Ok(Ok(true))
        )
    }
    pub async fn call(&self, request: &Request) -> anyhow::Result<Response> {
        let mut socket = UnixStream::connect(&self.path).await?;
        ensure!(same_user(&socket), "Automation broker owner mismatch");
        write(
            &mut socket,
            &Call {
                generation: Some(self.info.generation.clone()),
                request: Some(request.clone()),
            },
            MAX_REQUEST_BYTES + 1024,
        )
        .await?;
        let reply: Reply = read(&mut socket, MAX_RESPONSE_BYTES + 1024).await?;
        ensure!(
            reply.info.generation == self.info.generation
                && reply.info.fingerprint == self.info.fingerprint,
            "Desktop session changed"
        );
        let response = reply.response.context("Missing automation response")?;
        ensure!(
            response.version == 1 && response.operation_id == request.operation_id,
            "Automation response mismatch"
        );
        Ok(response)
    }
}
async fn read<T: DeserializeOwned>(socket: &mut UnixStream, limit: usize) -> anyhow::Result<T> {
    let len = socket.read_u32_le().await? as usize;
    ensure!(len <= limit, "Automation frame too large");
    let mut bytes = vec![0; len];
    socket.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid automation frame"))
}
async fn write<T: Serialize>(
    socket: &mut UnixStream,
    value: &T,
    limit: usize,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= limit, "Automation frame too large");
    socket.write_u32_le(bytes.len() as u32).await?;
    socket.write_all(&bytes).await?;
    Ok(())
}
