fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(9720);
    tokio::runtime::Runtime::new()?
        .block_on(cutlass_sync_server::run(([127, 0, 0, 1], port).into()))
}
