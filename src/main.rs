#[cfg(all(feature = "v2", feature = "v3"))]
compile_error!("Cannot enable both v2 and v3 features at the same time");

#[cfg(feature = "v2")]
#[tokio::main]
async fn main() {
    use ssw_v2::v2_main;
    v2_main().await
}

#[cfg(not(feature = "v2"))]
#[tokio::main]
async fn main() {
    println!("This binary was not compiled with the v2 feature enabled");
}
