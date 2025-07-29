// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

#[cfg(feature = "build-guest")]
mod build_guest {
    use risc0_build_ethereum::generate_solidity_files;

    // Paths where the generated Solidity files will be written.
    const SOLIDITY_IMAGE_ID_PATH: &str = "../../../contracts/src/libraries/PovwImageID.sol";
    const SOLIDITY_ELF_PATH: &str = "../../../contracts/test/PovwElf.sol";

    pub(super) fn build() {
        // Generate Rust source files for the methods crate.
        let guests = risc0_build::embed_methods();

        // Generate Solidity source files for use with Forge.
        let solidity_opts = risc0_build_ethereum::Options::default()
            .with_image_id_sol_path(SOLIDITY_IMAGE_ID_PATH)
            .with_elf_sol_path(SOLIDITY_ELF_PATH);

        if let Err(e) = generate_solidity_files(guests.as_slice(), &solidity_opts) {
            println!("cargo:warning=Failed to generate Solidity files: {}", e);
        };
    }
}

fn main() {
    #[cfg(feature = "build-guest")]
    build_guest::build();
}
