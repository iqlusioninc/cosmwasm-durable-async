# Native async v1 JSON fixtures

These shared wire fixtures pin the Rust/Go serialization boundary. Addresses are illustrative strings; these files test encoding, not address validation. Numeric coin amounts and Decimal gas prices are strings; heights, sequences and gas limits are JSON integers; Binary payloads are base64. Whitespace and object-key order are not significant.

The initial poll uses wait sequence 0. A newly created first wait uses sequence 1. Each resumed poll receives the consumed wait's sequence; an operation prepared by that poll uses the next sequence.

Consumers should decode into their actual protocol types and re-encode to semantically identical JSON. They must not compare two untyped JSON decoders and call that cross-language ABI validation.

The `task-*.json` files encode the public `TaskResponse`, including nullable execution fields and structured paused status. They intentionally replace backend-specific task storage JSON before release. Retention heights in fixtures are illustrative; each backend reports its actual retention policy.
