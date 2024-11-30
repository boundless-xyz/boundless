// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{env, fs, path::Path};

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

// NOTE: if alloy ever fixes https://github.com/alloy-rs/core/issues/688 this build script
// can be deleted and we should be able to just use the alloy::sol! macro
fn main() {
    println!("cargo::rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let sol_iface_path = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("contracts")
        .join("src")
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
            "{alloy_import}::sol! {{
            #![sol(all_derives)]
            {sol_contents}
}}
        "
        ),
    )
    .unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}
