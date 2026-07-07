# Integrity Certification

Result: **PASS**

| Check | Result | Details |
|---|---|---|
| old signature export+decode | PASS | 46 entries, 87.01 KiB |
| new signature export+decode | PASS | 47 entries, 87.75 KiB |
| old signature verify | PASS | every block hash matches the source |
| plan build+decode | PASS | CAVSPLAN1, 111 ops, payload 5.51 MiB (2.26 MiB on the wire) |
| no path traversal | PASS | 0 unsafe paths in 47 entries + 2 deletions |
| apply output byte-identical | PASS | verified against new signature in 213 ms (121.38 MiB from old, 5.51 MiB fresh) |
| no-op reapply | PASS | 0 files rewritten (44 detected as no-op); output byte-identical |
| corrupt signature rejected | PASS | decoder refused a bit-flipped .cavssig |
| corrupt plan rejected | PASS | decoder refused a bit-flipped .cavsplan |
| corrupted old input fails safely | PASS | a bit flipped inside a reused old range never produced a verified output |

Byte-identical reconstruction is mandatory: any hash mismatch fails the certification.
