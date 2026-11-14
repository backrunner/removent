//! Local automation attachment is not advertised until a same-user pipe broker exists.
use super::*;
use removent_proto::automation::{Request, Response};
#[derive(Clone)]
pub struct Info {
    pub generation: String,
    pub fingerprint: String,
    pub features: u64,
}
pub struct Publisher;
impl Publisher {
    pub fn start(_: &DataPaths, _: removent_net::RvpConnection, _: u64) -> anyhow::Result<Self> {
        anyhow::bail!("Local automation attachment is unavailable on Windows")
    }
}
pub struct Attachment {
    pub info: Info,
}
impl Attachment {
    pub async fn find(_: &DataPaths, _: [u8; 32]) -> anyhow::Result<Option<Self>> {
        Ok(None)
    }
    pub async fn alive(&self) -> bool {
        false
    }
    pub async fn call(&self, _: &Request) -> anyhow::Result<Response> {
        anyhow::bail!("Local automation attachment is unavailable on Windows")
    }
}
