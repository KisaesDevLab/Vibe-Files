# Fixtures

`cabinet/` is generated, not committed: `cargo run -p vf-fixtures` (seed 42 by default).
5 synthetic clients × 2 tax years × 6 one-page PDFs = 60 documents in Filer Desktop's default
template tree, each client folder bound by a real SentinelV1.

No real client data. SSNs are only 987-65-4320..4329 (SSA advertising range, never issued); EINs
use prefixes the IRS never assigned (07, 08, 09, 17, 18, 19, 28, 29). Every PDF says
"SYNTHETIC TEST DOCUMENT - NOT REAL DATA".

All PDFs have a text layer. Image-only scans (for P03's "EIN only in a scanned image" test) get
added to the generator in P03.
