#[cfg(all(not(feature = "dev"), feature = "v2"))]
#[tokio::main]
async fn main() {
    ssw_v2::v2_main().await
}

#[cfg(feature = "v3")]
#[tokio::main]
async fn main() {
    ssw_v3::v3_main()
}
