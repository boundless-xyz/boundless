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
