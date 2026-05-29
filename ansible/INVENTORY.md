# Inventory and Secret Management

The `prover-ansible` CodePipeline (`infra/pipelines/pipelines/l-prover-ansible.ts`)
deploys the provers with Ansible. To keep infrastructure details and credentials
out of GitHub, the runtime inventory, the SSH key, and the Tailscale auth key are
all stored in AWS Secrets Manager and injected into the CodeBuild environment at
run time. The `inventory.yml` checked into the repo is a **placeholder** — the
real values live only in Secrets Manager.

## Secrets used by the pipeline

| Secret name                          | Contents                                                        |
| ------------------------------------ | --------------------------------------------------------------- |
| `l-prover-ansible-private-key`       | SSH private key Ansible uses to reach the hosts                 |
| `l-prover-ansible-inventory-file`    | Base64-encoded `inventory.yml` (the real one, with credentials) |
| `l-prover-ansible-tailscale-authkey` | Ephemeral, tagged Tailscale auth key for CodeBuild              |

CodeBuild decodes the inventory secret into `ansible/inventory.yml` at build time
and removes it in `post_build`, so the placeholder in git is never used in CI.

## Updating the inventory secret

After changing the real inventory, push it to Secrets Manager base64-encoded:

```bash
base64 -i inventory.yml | \
  aws secretsmanager put-secret-value \
    --secret-id l-prover-ansible-inventory-file \
    --secret-string file:///dev/stdin
```

(Use `base64 -w0` on Linux to avoid line wrapping.) No pipeline re-deploy is
needed for an inventory value change — the next run picks it up.

## Tailscale: CI reaches the provers over the tailnet

CI no longer connects to provers on their public IPs. CodeBuild joins the tailnet
in userspace mode and SSHes through a local SOCKS5 proxy, so the inventory uses
[MagicDNS](https://tailscale.com/kb/1081/magicdns) names instead of IPs.

Host mapping (group → MagicDNS host):

| Group                            | MagicDNS host |
| -------------------------------- | ------------- |
| `prover_8453_production_release` | `prover-01`   |
| `prover_8453_production_nightly` | `prover-02`   |
| `prover_84532_staging_nightly`   | `prover-03`   |

> The `execution_*` (explorer) hosts are intentionally left on their public IPs;
> they are not part of the Tailscale move.

## One-time / out-of-band setup checklist

These steps are not code and must be done before the first pipeline run:

1. **Tailscale auth key (admin console):** create an **ephemeral**, **reusable**,
   **tagged** auth key (e.g. tag `tag:ci-prover`). Ephemeral so the CodeBuild node
   auto-removes when the build ends; reusable so every run can use it until it
   expires (rotate on/before expiry).
2. **Tailscale ACL:** allow `tag:ci-prover` to reach the prover hosts on `tcp:22`.
   Scope it to just the prover nodes; do not grant broad tailnet access.
3. **Join the provers to the tailnet** as `prover-01`, `prover-02`, `prover-03`
   (MagicDNS names) and confirm each is SSH-reachable over the tailnet from a
   tailnet-connected host.
4. **Seed the secrets:**
   - `l-prover-ansible-tailscale-authkey` = the auth key from step 1.
   - `l-prover-ansible-inventory-file` = base64 of the updated `inventory.yml`
     (see above).
5. **Re-deploy `infra/pipelines`** so the new secret + `TAILSCALE_AUTHKEY` env var
   wiring exists before the first Ansible run.

## Validation

- From a tailnet-connected host, confirm the provers are reachable over MagicDNS:
  `ansible -i inventory.yml prover -m ping --limit <group>`.
- Run a dry-run deploy from a tailnet-connected host:
  `ansible-playbook -i inventory.yml prover.yml --check --limit <group>`.
