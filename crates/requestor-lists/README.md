# Requestor Lists

Requestor Lists is a specification for listing Boundless Market requestors and associated metadata (e.g. chainId, address, ..).

Requestor Lists are intended to be flexible enough to be used by any application that needs to consume lists of requestors. Currently the Boundless Broker supports these lists for defining priority requestors.

## Overview

This crate provides a standardized JSON schema and Rust types for publishing lists of requestor addresses. The format is intentionally flexible to support multiple use cases, for example:

- **Priority lists** - Requestors that should be prioritized for processing
- **Allow lists** - Requestors that are permitted to submit requests
- **Deny lists** - Requestors that should be blocked

## Schema

### List Structure

```json
{
  "name": "string",
  "description": "string",
  "schemaVersion": { "major": 1, "minor": 0 },
  "version": { "major": 1, "minor": 0 },
  "requestors": [...]
}
```

### Requestor Entry

```json
{
  "address": "0x...",
  "chainId": 1,
  "name": "Requestor Name",
  "description": "Optional description",
  "tags": ["tag1", "tag2"],
  "extensions": {
    "priority": { "level": 75 },
    "requestEstimates": {
      "estimatedMcycleCountMin": 100,
      "estimatedMcycleCountMax": 2000,
      "estimatedMaxInputSizeMB": 25
    },
    "denylist": {
      "reason": "spam",
      "blockedSince": "2025-01-01T00:00:00Z"
    }
  }
}
```

All extension fields are optional. This allows custom metadata to be included for lists with different purposes:

- Priority lists typically use `priority` and `requestEstimates`
- Deny lists typically use `denylist`
- Allow lists may have minimal or no extensions

## Multi-Chain Support

Requestor list creators are free to create lists per-chain, or a single list that spans all chains. Each requestor entry contains a `chainId` field. A requestor list can contain the same Ethereum address multiple times with different `chainId` values:

```rust
vec![
    RequestorEntry {
        address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse()?,
        chain_id: 1,  // Mainnet
        name: "Requestor on Mainnet",
        // ...
    },
    RequestorEntry {
        address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse()?,
        chain_id: 8453,  // Base
        name: "Same Requestor on Base",
        // ...
    },
]
```

Consumers of the list can filter by `chain_id` to get only relevant entries for their chain.

## Versioning

### Schema Version (`schemaVersion`)

The version of the schema structure itself. Breaking changes to field names, types, or validation rules require a major version bump. Minor version bumps are for backward-compatible additions.

Consumers should reject lists with mismatched major versions:

```rust
if list.schema_version.major != CURRENT_SCHEMA_VERSION.major {
    return Err(ValidationError::InvalidSchemaVersion { ... });
}
```

### List Version (`version`)

The version of the list content. Increment when requestors are added, removed, or updated. This helps consumers track when lists have changed.

## JSON Schema

The formal JSON schema is defined in `requestor-list.v1.schema.json` and conforms to Draft-07. Use it to validate lists with standard JSON schema validators.

## Example Lists

See the repository's `requestor-lists/` directory for example lists:

- `boundless-priority-list.standard.json` - Priority list for moderate provers
- `boundless-priority-list.large.json` - Priority list for large provers

## Usage

### Parsing a List

```rust
use requestor_lists::RequestorList;

// From JSON string
let list = RequestorList::from_json(json_str)?;

// From URL (async)
let list = RequestorList::fetch_from_url("https://example.com/list.json").await?;

// Validate
list.validate()?;
```

### Creating a List

```rust
use requestor_lists::{RequestorList, RequestorEntry, Version, Extensions};

let list = RequestorList::new(
    "My Priority List".to_string(),
    "High-priority requestors for my broker".to_string(),
    Version { major: 1, minor: 0 },
    vec![
        RequestorEntry {
            address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab".parse()?,
            chain_id: 1,
            name: "Example Requestor".to_string(),
            description: Some("A trusted requestor".to_string()),
            tags: vec!["defi".to_string()],
            extensions: Extensions::default(),
        }
    ],
);

let json = list.to_json()?;
```

### Accessing Metadata

```rust
for entry in &list.requestors {
    println!("Requestor: {} ({})", entry.name, entry.address);

    if let Some(priority) = &entry.extensions.priority {
        println!("  Priority level: {}", priority.level);
    }

    if let Some(estimates) = &entry.extensions.request_estimates {
        println!("  Estimated mcycles: {}-{}",
            estimates.estimated_mcycle_count_min,
            estimates.estimated_mcycle_count_max);
    }

    if let Some(denylist) = &entry.extensions.denylist {
        println!("  BLOCKED: {}", denylist.reason.as_deref().unwrap_or("No reason"));
    }
}
```
