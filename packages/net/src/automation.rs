//! One bounded request/response per automation stream. The session's single
//! bidi acceptor demultiplexes AUT1 and CLP1 after authentication completes.
use crate::{NetError, Result, RvpConnection};
use removent_proto::automation::{self, Request, Response};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

pub struct Incoming {
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
}

pub async fn read<T: DeserializeOwned>(recv: &mut quinn::RecvStream, limit: usize) -> Result<T> {
    let bytes = recv
        .read_to_end(limit)
        .await
        .map_err(|_| NetError::Framing("invalid or oversized automation message".into()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| NetError::Framing("invalid automation message".into()))
}

pub async fn write<T: Serialize>(
    send: &mut quinn::SendStream,
    value: &T,
    limit: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| NetError::Framing("cannot serialize automation message".into()))?;
    if bytes.len() > limit {
        return Err(NetError::Framing("automation result exceeds limit".into()));
    }
    crate::session::write_media(send, &bytes).await?;
    send.finish().map_err(|e| NetError::Framing(e.to_string()))
}

pub async fn call(conn: &RvpConnection, request: &Request) -> Result<Response> {
    request
        .validate()
        .map_err(|e| NetError::Rejected(e.to_string()))?;
    let work = async {
        let (mut send, mut recv) = conn
            .inner()
            .open_bi()
            .await
            .map_err(|e| NetError::Framing(e.to_string()))?;
        let _ = send.set_priority(-5);
        send.write_all(automation::MAGIC)
            .await
            .map_err(|e| NetError::Framing(e.to_string()))?;
        write(&mut send, request, automation::MAX_REQUEST_BYTES).await?;
        let response: Response = read(&mut recv, automation::MAX_RESPONSE_BYTES).await?;
        if response.version != automation::VERSION || response.operation_id != request.operation_id
        {
            return Err(NetError::Rejected(
                "automation response identity mismatch".into(),
            ));
        }
        Ok(response)
    };
    tokio::time::timeout(Duration::from_secs(30), work)
        .await
        .map_err(|_| NetError::Timeout)?
}
