# Release process

Our current versioning and release process is intended to facilitate easy releases, and a focus on moving the system forward over API stability or backporting of fixes.
As we approach a 1.0 release, these goals will be revised.

Releases are made from a tagged commit on the `main` branch.
When publishing a new release, you can use the following process.

1. Pick the new version number.

   If significant new features or breaking changes are included, bump the version number should be `v0.x+1.0`.
   Note that this will be most releases.
   If the release contains only fixes to the latest release, the version number should be `v0.x.y+1`.

2. Build the assessor guest.

   ```zsh
   cargo risczero build --manifest-path crates/guest/assessor/assessor-guest/Cargo.toml
   ```

   This will output the image ID and file location.

   If the image ID has changed relative to what's recorded in `deployment.toml`:

   1. Upload the ELF to some public HTTP location (such as Pinata), and get back a download URL.
   2. Record these values in `deployment.toml` as `assessor-image-id` and `assessor-guest-url`.

3. If the version number or `deployment.toml` need to be updated, open a PR to `main` to do so.

   Run the [release workflow][release-workflow] against this branch to confirm the assessor image ID is stable and recorded correctly.

   Note that the `deployment` job in the workflow will fail.

   > TODO: Update the release workflow to be able to run pre-release checks by executing the upgrade against an Anvil fork instance, then running the deployment tests to confirm functionality.

   Merge this PR before executing the next step.
   The commit that results will be target commit for the release.

4. Follow the instructions in the [contracts](./contracts/scripts/README.md) directory to upgrade the contract on Sepolia from the target commit.

   > TODO: Upgrading the contract this way may be disruptive. We need a better process here.

   If the contract addresses changed, update [deployments.mdx](./documentation/site/pages/deployments.mdx)
   Additionally search for the old address, and replace any occurrences elsewhere in the documentation.

5. Tag the target commit as `v0.x.y`, as chosen in step one.
   Push the tag with `git push origin v0.x.y`.

   When the tag is pushed, a run of the [release workflow][release-workflow] will kick off against that tag.
   Watch and confirm that all tests pass, and take action if they do not.

6. Publish the new version of the crates to [crates.io](https://crates.io).

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

7. Open a PR to bump the development version on `main` to `v0.x+1.0`.

[release-workflow]: https://github.com/boundless-xyz/boundless/actions/workflows/release.yml
