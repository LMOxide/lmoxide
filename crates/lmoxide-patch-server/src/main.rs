#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lmoxide_patch_server::run_cli().await
}
