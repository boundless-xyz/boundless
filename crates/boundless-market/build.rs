// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{env, fs, fs::File, io::Write, path::Path};

fn insert_derives(contents: &mut String, find_str: &str, insert_str: &str) {
    let mut cur_pos = 0;
    while let Some(struct_pos) =
        contents.match_indices(find_str).find_map(|(i, _)| (i >= cur_pos).then_some(i))
    {
        // println!("cargo:warning={struct_pos}");
        contents.insert_str(struct_pos, insert_str);
        cur_pos = struct_pos + insert_str.len() + find_str.len();
    }
}

// NOTE: if alloy ever fixes https://github.com/alloy-rs/core/issues/688 this function
// can be deleted and we should be able to just use the alloy::sol! macro
fn rewrite_solidity_interface_files() {
    println!("cargo::rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let sol_iface_path = Path::new(&manifest_dir)
        .join("src")
        .join("contracts")
        .join("artifacts")
        .join("IBoundlessMarket.sol");

    println!("cargo::rerun-if-changed={}", sol_iface_path.to_string_lossy());

    let mut sol_contents = fs::read_to_string(sol_iface_path).unwrap();

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    // skip the sol(rpc) insert if building for the zkvm
    let mut alloy_import = "alloy_sol_types";
    if target_os != "zkvm" {
        let iface_pos = sol_contents.find("interface IBoundlessMarket").unwrap();
        sol_contents.insert_str(iface_pos, "#[sol(rpc)]\n");
        alloy_import = "alloy";
    }

    insert_derives(&mut sol_contents, "\nstruct ", "\n#[derive(Deserialize, Serialize)]");
    insert_derives(&mut sol_contents, "\nenum ", "\n#[derive(Deserialize, Serialize)]");

    println!("cargo::rerun-if-env-changed=OUT_DIR");
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("boundless_market.rs");
    fs::write(
        dest_path,
        format!(
            "#[allow(missing_docs)]
        pub mod boundless_market_contract {{
            use serde::{{Deserialize, Serialize}};
            {alloy_import}::sol! {{
            #![sol(all_derives)]
            {sol_contents}
}}
}}
        "
        ),
    )
    .unwrap();
}

fn copy_interfaces() {
    let target_contracts = ["IBoundlessMarket"];

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dest_path = Path::new(&manifest_dir).join("src/contracts/artifacts");
    fs::create_dir_all(&dest_path).unwrap();

    let src_path =
        Path::new(&manifest_dir).parent().unwrap().parent().unwrap().join("contracts").join("src");

    for contract in target_contracts {
        let source_path = src_path.join(format!("{contract}.sol"));
        // Tell cargo to rerun if this contract changes
        println!("cargo:rerun-if-changed={}", source_path.display());

        if source_path.exists() {
            // Copy the file to the destination
            std::fs::copy(&source_path, dest_path.join(format!("{contract}.sol"))).unwrap();
        }
    }
}

fn copy_artifacts() {
    let target_contracts =
        ["BoundlessMarket", "RiscZeroMockVerifier", "RiscZeroSetVerifier", "ERC1967Proxy"];

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dest_path = Path::new(&manifest_dir).join("src/contracts/artifacts");
    fs::create_dir_all(&dest_path).unwrap();

    let src_path =
        Path::new(&manifest_dir).parent().unwrap().parent().unwrap().join("contracts").join("out");

    for contract in target_contracts {
        let source_path = src_path.join(format!("{contract}.sol/{contract}.json"));
        // Tell cargo to rerun if this contract changes
        println!("cargo:rerun-if-changed={}", source_path.display());

        if source_path.exists() {
            // Read and parse the JSON file
            let json_content = fs::read_to_string(&source_path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&json_content).unwrap();

            // Extract the bytecode, removing "0x" prefix if present
            let bytecode = json["bytecode"]["object"]
                .as_str()
                .ok_or(format!(
                    "failed to extract bytecode from {}",
                    source_path.as_os_str().to_string_lossy()
                ))
                .unwrap()
                .trim_start_matches("0x");

            // Write to new file with .hex extension
            let dest_file = dest_path.join(format!("{contract}.hex"));
            let mut file = File::create(dest_file).unwrap();
            file.write_all(bytecode.as_bytes()).unwrap();
        }
    }
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    copy_interfaces();
    rewrite_solidity_interface_files();

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os != "zkvm" {
        copy_artifacts();
    }
}
