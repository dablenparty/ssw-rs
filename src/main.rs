#[cfg(all(feature = "v2", feature = "v3"))]
compile_error!("Cannot enable both v2 and v3 features at the same time");

#[cfg(all(not(feature = "v2"), not(feature = "v3")))]
compile_error!("Must enable either v2 or v3 feature");

#[cfg(feature = "v2")]
#[tokio::main]
async fn main() {
    ssw_v2::v2_main().await
}

#[cfg(not(feature = "v2"))]
#[tokio::main]
async fn main() {
    ssw_v3::v3_main()
}
