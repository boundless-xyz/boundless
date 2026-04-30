# boundless-executor

An HTTP service that executes guest programs across multiple zkVMs and reports
cycle counts. The API is wire-compatible with [`bonsai-sdk`](https://docs.rs/bonsai-sdk),
so any client written against Bonsai can target this service with nothing more
than a URL change.

Useful for profiling, benchmarking, and sanity-checking guests before sending
them to a prover.

## Features

The crate is a standalone Cargo workspace so each zkVM SDK can be pulled in
independently via feature flags.

| feature | SDK crate    | default | runtime requirement     |
| ------- | ------------ | ------- | ----------------------- |
| `risc0` | `risc0-zkvm` | ✅      | `r0vm` binary on `PATH` |

The RISC Zero backend uses the `client` feature of `risc0-zkvm` (not `prove`)
and shells out to `r0vm` for execution — it keeps the build lean but means
`r0vm` must be installed:

```bash
rzup install r0vm
# …or set RISC0_SERVER_PATH=/path/to/r0vm explicitly
```

### Adding another zkVM

The `Executor` trait + `Registry` are deliberately backend-agnostic so more
zkVMs can be slotted in later:

1. Add a variant to `ZkvmKind` in [`src/backend.rs`](src/backend.rs).
2. Add a feature flag and (optional) SDK dependency to `Cargo.toml`.
3. Create `src/backends/<name>.rs` implementing `Executor`.
4. Register it in [`default_registry`](src/backends/mod.rs) behind the new
   feature flag.

No changes to the API layer are required — clients pick the backend by name
via the `zkvm` field in `POST /sessions/create`.

## Bonsai-compatible HTTP API

The server mirrors the Bonsai REST surface. Every endpoint below matches the
path, method, headers (`x-api-key`, `x-risc0-version`), and JSON shape the
stock SDK expects. Only the **execute-only** flow is supported — proving
requests are rejected, and the `/snark/*` endpoints return 400.

### Session lifecycle

| method | path                                 | purpose                                      |
| ------ | ------------------------------------ | -------------------------------------------- |
| GET    | `/version`                           | list supported `risc0-zkvm` versions         |
| GET    | `/user/quotas`                       | returns permissive stub quotas               |
| GET    | `/images/upload/{image_id}`          | returns presigned PUT url (or 204 if exists) |
| PUT    | `/images/upload/{image_id}`          | store image bytes (ELF / R0BF ProgramBinary) |
| DELETE | `/images/{image_id}`                 | remove an image                              |
| GET    | `/inputs/upload`                     | returns `{ uuid, url }` for PUT              |
| PUT    | `/inputs/upload/{uuid}`              | store input bytes                            |
| DELETE | `/inputs/{uuid}`                     | remove an input                              |
| GET    | `/receipts/upload`                   | (accepted; assumptions are no-ops)           |
| PUT    | `/receipts/upload/{uuid}`            | (accepted; assumptions are no-ops)           |
| POST   | `/sessions/create`                   | create an execute-only session               |
| GET    | `/sessions/status/{uuid}`            | poll status: `RUNNING`/`SUCCEEDED`/`FAILED`  |
| GET    | `/sessions/logs/{uuid}`              | human-readable session log                   |
| GET    | `/sessions/stop/{uuid}`              | abort a running session                      |
| GET    | `/sessions/exec_only_journal/{uuid}` | raw journal bytes (once `SUCCEEDED`)         |

Meta:

| method | path      | purpose        |
| ------ | --------- | -------------- |
| GET    | `/health` | liveness probe |

### `POST /sessions/create`

```json
{
  "img": "<hex image_id>",
  "input": "<input uuid>",
  "assumptions": [],
  "execute_only": true,
  "exec_cycle_limit": null,
  "zkvm": "risc0"
}
```

`zkvm` is a **non-standard extension** — omit it and you get `"risc0"` for
Bonsai SDK compatibility. Additional backends registered via the `Executor`
trait can be selected by name.

Response:

```json
{ "uuid": "b3b9…" }
```

### `GET /sessions/status/{uuid}`

```json
{
  "status": "SUCCEEDED",
  "receipt_url": "http://localhost:8080/sessions/exec_only_journal/b3b9…",
  "state": null,
  "elapsed_time": 0.412,
  "stats": {
    "segments": 0,
    "total_cycles": 131072,
    "cycles": 95234
  }
}
```

`stats.cycles` is the user cycle count, `stats.total_cycles` the padded
proving cycles. `segments` is currently always `0` — the Executor trait
doesn't yet surface the per-segment breakdown.

## Using the `bonsai-sdk` client

```rust
use bonsai_sdk::blocking::Client;
use risc0_zkvm::compute_image_id;

let client = Client::from_parts(
    "http://localhost:8080".into(),
    "dev".into(),      // any string — auth is not enforced
    "3.0.4",
)?;

let elf: &[u8] = /* your R0BF ProgramBinary */;
let image_id = hex::encode(compute_image_id(elf)?);
client.upload_img(&image_id, elf.to_vec())?;

let input_id = client.upload_input(input_bytes)?;

let session = client.create_session(image_id, input_id, vec![], true /* execute_only */)?;
loop {
    let res = session.status(&client)?;
    if res.status == "RUNNING" {
        std::thread::sleep(std::time::Duration::from_millis(200));
        continue;
    }
    println!("{:?}", res.stats);
    break;
}
let journal = session.exec_only_journal(&client)?;
```

Point `BONSAI_API_URL=http://localhost:8080` and `BONSAI_API_KEY=dev` and any
existing Bonsai-based tooling should run against this service unchanged.

## Testing with `curl`

```bash
# 1) upload an image (R0BF ProgramBinary, e.g. target/.../echo.bin)
IMG=$(xxd -p -c0 echo.bin)
IMG_ID=$(openssl dgst -sha256 -binary echo.bin | xxd -p -c0 | cut -c1-64)  # demo only
PRESIGN=$(curl -s "localhost:8080/images/upload/$IMG_ID" | jq -r .url)
curl -s -X PUT --data-binary @echo.bin "$PRESIGN"

# 2) upload input
read UUID URL < <(curl -s localhost:8080/inputs/upload | jq -r '.uuid, .url')
printf 'hello' | curl -s -X PUT --data-binary @- "$URL"

# 3) create session
SESSION_UUID=$(curl -s -X POST localhost:8080/sessions/create \
  -H 'content-type: application/json' \
  -d "{\"img\":\"$IMG_ID\",\"input\":\"$UUID\",\"assumptions\":[],\"execute_only\":true}" \
  | jq -r .uuid)

# 4) poll status
curl -s "localhost:8080/sessions/status/$SESSION_UUID" | jq

# 5) fetch journal
curl -s "localhost:8080/sessions/exec_only_journal/$SESSION_UUID" | xxd -c0
```

## Configuration

| env var               | flag           | default              |
| --------------------- | -------------- | -------------------- |
| `EXECUTOR_BIND`       | `--bind`       | `0.0.0.0:8080`       |
| `EXECUTOR_PUBLIC_URL` | `--public-url` | `http://{bind}`      |
| `EXECUTOR_BODY_LIMIT` | `--body-limit` | `268435456` (256MiB) |
| `RUST_LOG`            | —              | `info`               |

`--public-url` is the base URL the server advertises in the presigned upload
responses. Set it when running behind a reverse proxy or in docker so clients
can reach the `PUT` endpoint they're handed back.

## Notes on cycle reporting

- **RISC Zero**: `cycles` = sum of per-segment user cycles; `total_cycles` =
  sum of `2^po2` per segment (padded proving cycles). Values come from
  `r0vm`'s `SessionInfo`. Input must be an R0BF **ProgramBinary** (the `.bin`
  files emitted by `risc0-build`), not a raw RISC-V ELF.

## Errors

Errors return JSON of the form `{ "error": "<message>" }`:

- `400` — backend disabled, unknown image/input id, `execute_only=false`,
  malformed payload, or proving (snark) requests
- `404` — unknown session id, or requesting a receipt (execute-only has none)
- `500` — executor failure
