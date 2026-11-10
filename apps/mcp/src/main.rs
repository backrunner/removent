//! Stdio MCP entry point. Stdout is reserved exclusively for MCP JSON-RPC.
use rmcp::ServiceExt;
mod server;
mod sessions;
mod tools;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = removent_core::DataPaths::resolve();
    paths.ensure_layout()?;
    let settings = removent_core::Settings::load_strict(&paths)?;
    let identity = removent_core::identity::load_or_create(&paths, &settings.device_name)?;
    let state = std::sync::Arc::new(sessions::Sessions::new(paths, settings, identity));
    let server = server::Server(state.clone());
    let service = server.serve(rmcp::transport::stdio()).await?;
    let result = service.waiting().await;
    state.shutdown();
    result?;
    Ok(())
}
