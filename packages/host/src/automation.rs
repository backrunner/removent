//! Authenticated-session RPC. Authorization is resolved by the host runner.
use removent_automation::Executor;
use removent_net::automation::{Incoming, read, write};
use removent_proto::automation::{self, Failure, Request, Response};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) fn serve(
    mut incoming: mpsc::Receiver<Incoming>,
    executor: Option<Executor>,
    session: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! { biased;
                _ = session.cancelled() => break,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                stream = incoming.recv(), if tasks.len() < 8 => {
                    let Some(mut stream) = stream else { break };
                    let executor = executor.clone();
                    let cancel = session.child_token();
                    tasks.spawn(async move {
                        let _guard = cancel.clone().drop_guard();
                        let request = tokio::time::timeout(Duration::from_secs(5), read::<Request>(&mut stream.recv, automation::MAX_REQUEST_BYTES)).await;
                        let Ok(Ok(request)) = request else { return };
                        let operation_id = request.operation_id.clone();
                        let result = match request.validate() {
                            Err(error) => Err(error),
                            Ok(()) => match executor {
                                None => Err(Failure::new("permission_denied", "Host has not granted Computer Use to this client certificate, or the native worker is unavailable")),
                                Some(executor) => tokio::select! { biased;
                                    _ = cancel.cancelled() => return,
                                    _ = stream.send.stopped() => return,
                                    result = tokio::time::timeout(Duration::from_secs(25), executor.execute(request,cancel.clone())) => result.unwrap_or_else(|_| Err(Failure::new("outcome_unknown","Native operation timed out; do not repeat with a new operation ID"))),
                                },
                            },
                        };
                        // Stop a timed-out native gesture before attempting a possibly blocked reply.
                        cancel.cancel();
                        let response = Response { version: automation::VERSION, operation_id, result };
                        let _ = tokio::time::timeout(Duration::from_secs(5),write(&mut stream.send,&response,automation::MAX_RESPONSE_BYTES)).await;
                    });
                }
            }
        }
    })
}
