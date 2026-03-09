fn main() {
    #[cfg(feature = "build-guest")]
    risc0_build::embed_methods();
}
