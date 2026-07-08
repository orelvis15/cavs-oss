# Traffic report

Model **adjacent-heavy**, 100000 users, expanded to 35 weighted (from,to) queries over 10 versions.

| Rule | Probability |
|---|---:|
| adjacent | 80% |
| skip_range 2–4 | 15% |
| old_to_latest (age ≥ 6) | 4% |
| reinstall_latest | 1% |

| From | To | Rule | Probability |
|---|---|---|---:|
| v01 | v02 | adjacent | 8.889% |
| v01 | v03 | skip_range | 0.714% |
| v01 | v04 | skip_range | 0.714% |
| v01 | v05 | skip_range | 0.714% |
| v01 | v10 | old_to_latest | 1.000% |
| v02 | v03 | adjacent | 8.889% |
| v02 | v04 | skip_range | 0.714% |
| v02 | v05 | skip_range | 0.714% |
| v02 | v06 | skip_range | 0.714% |
| v02 | v10 | old_to_latest | 1.000% |
| v03 | v04 | adjacent | 8.889% |
| v03 | v05 | skip_range | 0.714% |
| v03 | v06 | skip_range | 0.714% |
| v03 | v07 | skip_range | 0.714% |
| v03 | v10 | old_to_latest | 1.000% |
| v04 | v05 | adjacent | 8.889% |
| v04 | v06 | skip_range | 0.714% |
| v04 | v07 | skip_range | 0.714% |
| v04 | v08 | skip_range | 0.714% |
| v04 | v10 | old_to_latest | 1.000% |
| v05 | v06 | adjacent | 8.889% |
| v05 | v07 | skip_range | 0.714% |
| v05 | v08 | skip_range | 0.714% |
| v05 | v09 | skip_range | 0.714% |
| v06 | v07 | adjacent | 8.889% |
| v06 | v08 | skip_range | 0.714% |
| v06 | v09 | skip_range | 0.714% |
| v06 | v10 | skip_range | 0.714% |
| v07 | v08 | adjacent | 8.889% |
| v07 | v09 | skip_range | 0.714% |
| v07 | v10 | skip_range | 0.714% |
| v08 | v09 | adjacent | 8.889% |
| v08 | v10 | skip_range | 0.714% |
| v09 | v10 | adjacent | 8.889% |
| v10 | v10 | reinstall_latest | 1.000% |
