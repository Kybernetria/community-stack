# Reticulum sync protocol v1 contract

Status: **wire design; operation transfer is disabled in the included bridge until golden fixtures and inbound authorization pass.**

## Rules

- Establish authenticated/encrypted Reticulum Links. Optional initiator identification is policy-controlled. The baseline sidecar caps active Links at 32, evicts links idle for five minutes, and rejects malformed bounded Hello payloads.
- Query each Link Channel's negotiated `mdu`; never assume 465 bytes or any fixed payload capacity.
- MessagePack control messages must fit one Channel message and declare protocol/schema limits.
- Pull data. Gossip/announces only indicate availability.
- Never decode Loro, decide membership, decrypt application records, or emit application ACKs in the bridge.
- Every count, length, page, range, bundle, session, and decompression result is bounded.

## Handshake

```text
Hello {
  protocol_versions: [u16],
  schema_versions: [u32],
  codecs: [raw, zstd],
  max_control_frame: u16,
  max_bundle: u32,
  profile: radio | balanced | fast
}

HelloAck {
  selected versions/codecs,
  min(local Channel MDU, offered control max),
  selected bundle max,
  profile,
  operation_transfer: bool
}
```

No further message is accepted until negotiation succeeds.

## Exact anti-entropy

```text
ManifestPage {
  session_id, topic,
  heads: [{author, log_id, generation, sequence, operation_hash}],
  checkpoints: [checkpoint_id],
  next_cursor?
}

NeedRanges {
  session_id,
  ranges: [{author, log_id, generation, after, until}],
  byte_budget
}

BundleOffer {
  session_id, bundle_id, operation_count, encoded_bytes, bundle_hash
}
BundleAccept { session_id, bundle_id, accepted_byte_budget }
BundleCancel { session_id, bundle_id, reason_code }

DurableAck {
  session_id,
  items: [{operation_hash, status: STORED|APPLIED|PENDING_DEPS|REJECTED, reason_code?}]
}
```

Manifest pages and requests use Channel. Small records may use Channel only when the complete encoded message fits negotiated MDU. Accepted bounded operation bundles/checkpoints use Resource with `auto_compress=False`, because application ciphertext is already conditionally compressed before encryption.

A bundle is a length-delimited sequence of `(canonical_header, ciphertext_body)`. Bundle hash is BLAKE3 over the entire encoded bundle. Declared and actual lengths must agree before individual records enter the core inbox.

## Scheduling defaults

| Class | Priority | Radio default |
|---|---:|---|
| auth removal / key epoch control | 10 | enabled |
| ACK / dependency request | 20 | enabled |
| compact domain message | 30 | enabled |
| Loro update | 40 | enabled |
| checkpoint | 50 | explicit budget |
| blob | 60 | disabled |

Token buckets exist per Reticulum interface, community/topic, peer, and class. Exact paged reconciliation runs periodically even if probabilistic summaries report no difference.

## Interoperability gates

1. Python↔Rust fixtures for every control frame and malformed variant.
2. Runtime assertion that frames fit actual Channel MDU across supported RNS profiles.
3. Duplicate, reordered, truncated, oversized, interrupted Resource, and reconnect tests.
4. Remote core ACK is recorded only after its SQLite commit.
5. Queues, leases, disk usage, retries, and airtime stay bounded under a 72-hour partition.
6. Hardware tests at the slowest supported radio profile.
7. Native Reticulum alternatives must interoperate against the pinned Python implementation before replacement.
