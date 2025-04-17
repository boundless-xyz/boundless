# Release process

Our current versioning and release process is intended to facilitate easy releases, and a focus on moving the system forward over API stability or backporting of fixes.
As we approach a 1.0 release, these goals will be revised.

> [!IMPORTANT]
> Cut new releases only after all related changes have been deployed and validated on the staging environment.

Releases are made from a tagged commit on the `release-x.y.z` branch.
When publishing a new release, you can use the following process.

## Update staging

1. Pick the new version number.

   If significant new features or breaking changes are included, bump the version number should be `v0.x+1.0`.
   Note that this will be most releases.
   If the release contains only fixes to the latest release, the version number should be `v0.x.y+1`.

2. Build the assessor guest.

   ```zsh
   cargo risczero build --manifest-path crates/guest/assessor/assessor-guest/Cargo.toml
   ```

   > [!TIP]
   > Make sure to install `cargo risczero` with the `experimental` feature.

   This will output the image ID and file location.

   If the image ID has changed relative to what's recorded in `deployment.toml` under the `[chains.ethereum-sepolia-staging]` section:

   1. Upload the ELF to some public HTTP location (such as Pinata), and get back a download URL.
   2. Record these values in `deployment.toml` as `assessor-image-id` and `assessor-guest-url` under the `[chains.ethereum-sepolia-staging]` section.

3. If a new version of the `RiscZeroSetVerifier` contract has been previously deployed, make sure to reflect the latest address on the `deployment.toml` file.

4. Follow the instructions in the [contracts](./contracts/scripts/README.md) directory to upgrade the contract on Sepolia from the target commit.

   > [!NOTE]
   > If the contract has breaking changes, a new deployment will be required. Otherwise, just upgrade.
   > In both cases, use `ethereum-sepolia-staging` as `CHAIN_KEY` env variable.

   If the contract addresses changed due to a new deployment, update the relevant values in `deployment.toml`.

5. Run the [release workflow][release-workflow] manually against this branch using `ethereum-sepolia-staging` as `chain_key` to confirm the deployment is working correctly.

   If the workflow fails, make sure that the [Deployment.t.sol](./contracts/deployment-test/Deploymnet.t.sol) file is up-to-date with:
   - The content of the request (e.g., image ID, imageUrl, input, requirements)
   - The selector used matches with the zkvm version (i.e., search for "selector for ZKVM_V"); if not, update its value as listed in [selector.rs](./lib/risc0-ethereum/contracts/src/selector.rs).

## Update prod

> [!NOTE]
> Only run these steps after the update on staging is successful.

1. Copy the changes made on `assessor-image-id` and `assessor-guest-url` on the `deployment.toml` file for staging under the `[chains.ethereum-sepolia-prod]` section.

2. Follow the instructions in the [contracts](./contracts/scripts/README.md) directory to upgrade the contract on Sepolia from the target commit.

   > TODO: Upgrading the contract this way may be disruptive. We need a better process here.

   If the contract addresses changed, update [deployments.mdx](./documentation/site/pages/developers/smart-contracts/deployments.mdx)
   Additionally search for the old address, and replace any occurrences elsewhere in the documentation.

3. If the version number or `deployment.toml` need to be updated, open a PR to `main` to do so.

   Run the [release workflow][release-workflow] manually against this branch using `ethereum-sepolia-prod` as `chain_key` (default value) to confirm the deployment is working correctly.

   Merge this PR before executing the next step.
   The commit that results will be target commit for the release.

4. Cut a new release as `v0.x.y`, as chosen in step one of the [update staging](#update-staging) section.

   When the new release tag is pushed, a run of the [release workflow][release-workflow] will kick off against that tag.
   Watch and confirm that all tests pass, and take action if they do not.

5. Publish the new version of the crates to [crates.io](https://crates.io).

   The currently published crates are:

   - `boundless-market`
   - `boundless-assessor`
   - `boundless-cli`

   > NOTE: When publishing a new crate, make sure to add github:risc0:maintainers as an owner.

   <br/>

   ```sh
   # Log in to crates.io. Create a token that is restricted to what you need to do (e.g. publish update) and set an expiry.
   cargo login
   # Dry run to check that the package will publish. Look through the output, e.g. at version numbers, to confirm it makes sense.
   cargo publish -p $PKG --dry-run
   # Actually publish the crate
   cargo publish -p $PKG
   ```

6. Open a PR to bump the development version on `main` to `v0.x+1.0`.

[release-workflow]: https://github.com/boundless-xyz/boundless/actions/workflows/release.yml

7. Inform all the stakeholders through the relevant channels
