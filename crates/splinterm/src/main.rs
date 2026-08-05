mod app;

#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    app::run().await
}
