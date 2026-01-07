// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

mod build_contracts {
    use std::{env, fs, path::Path};

    // Contract interface files to copy to the artifacts folder
    const ZKC_INTERFACE_FILES: [&str; 4] =
        ["IStaking.sol", "IRewards.sol", "IZKC.sol", "IVotes.sol"];
    const INTERFACE_FILES: [&str; 1] = ["IStakingRewards.sol"];

    /// Copy contract interface files from contracts/src/povw to src/contracts/artifacts
    fn copy_contract_interfaces() {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let zkc_contracts_src = Path::new(&manifest_dir)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("lib/zkc/src/interfaces");
        let contracts_src =
            Path::new(&manifest_dir).parent().unwrap().parent().unwrap().join("contracts/src/zkc");

        // Early return if contracts source doesn't exist (enables cargo publish)
        if !zkc_contracts_src.exists() {
            return;
        }
        if !contracts_src.exists() {
            return;
        }

        let artifacts_dir = Path::new(&manifest_dir).join("src/contracts/artifacts");
        fs::create_dir_all(&artifacts_dir).unwrap();

        println!("cargo:rerun-if-changed={}", zkc_contracts_src.display());

        for interface_file in ZKC_INTERFACE_FILES {
            let src_path = zkc_contracts_src.join(interface_file);
            let dest_path = artifacts_dir.join(interface_file);

            println!("cargo:rerun-if-changed={}", src_path.display());

            if src_path.exists() {
                fs::copy(&src_path, &dest_path).unwrap();
            }
        }
        for interface_file in INTERFACE_FILES {
            let src_path = contracts_src.join(interface_file);
            let dest_path = artifacts_dir.join(interface_file);

            println!("cargo:rerun-if-changed={}", src_path.display());

            if src_path.exists() {
                fs::copy(&src_path, &dest_path).unwrap();
            }
        }
    }

    pub(super) fn build() {
        copy_contract_interfaces();
        // generate_bytecode_module();
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_contracts::build();
}
