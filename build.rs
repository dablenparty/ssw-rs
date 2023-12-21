#[cfg(all(feature = "dev", not(debug_assertions)))]
compile_error!("dev feature is only allowed in debug mode");

#[cfg(all(not(feature = "dev"), feature = "v2", feature = "v3"))]
compile_error!("Cannot enable both v2 and v3 features at the same time");

#[cfg(all(not(feature = "v2"), not(feature = "v3")))]
compile_error!("Must enable either v2 or v3 feature");

fn main() {
    #[cfg(feature = "dev")]
    println!("cargo:warning=DEV MODE ENABLED, ARTIFACTS WILL BE LARGE");

    println!("cargo:rerun-if-changed=build.rs");
}