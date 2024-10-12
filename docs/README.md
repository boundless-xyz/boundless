# Boundless Book

## Dependencies

```sh
cargo install mdbook
```

## Serve the docs locally

```sh
mdbook serve --open -p 3001
```

## Linting & Formatting

From the top-level working directory:

```sh
# Format all files configured in .dprint.jsonc
dprint fmt
# Check all links configured in lychee.toml
lychee .
```
