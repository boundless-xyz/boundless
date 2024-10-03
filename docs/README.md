# Boundless Book

## Dependencies

```console
cargo install mdbook
```

## Serve the docs locally

```console
mdbook serve --open -p 3001
```

## Linting & Formatting

From the top-level working directory:

```console
# Format all files configured in .dprint.jsonc
dprint fmt
# Check all links configured in lychee.toml
lychee .
```

To check all reference links that are included from [links.md](./src/reference/links.md) and otherwise missed by Lychee [^1]:

```console
# Generate HTML with all links
mdbook build docs
# Check all links configured in lychee.toml, but include build artifacts
lychee docs/book --exclude-path override-config
```

Note that all files are at the same paths, but in `src` not `book` and `README.md` is remapped to `index.html`.
All link issues should be resolved by updating [links.md](./src/reference/links.md)

[^1]: Should not be required soon™️ (we hope): https://github.com/lycheeverse/lychee/issues/456 <!-- TODO less manual checks on links -->
