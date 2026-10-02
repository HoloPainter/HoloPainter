# Third-party license texts

This directory contains license texts for the bundled assets and Zstandard identified in `../THIRD_PARTY_NOTICES.md`.

| File | Component | Original source |
| --- | --- | --- |
| `NotoSansJP-OFL.txt` | Noto Sans JP (SIL OFL 1.1) | https://github.com/notofonts/noto-cjk/blob/main/Sans/LICENSE |
| `Apache-2.0.txt` | Google Material Symbols / Material Icons (Apache 2.0) | https://github.com/google/material-design-icons/blob/master/LICENSE |
| `Zstandard-BSD-3-Clause.txt` | Zstandard 1.5.7 (BSD 3-Clause) | `zstd-sys` 2.0.16+zstd.1.5.7 source package, `zstd/LICENSE` |

Asset copyright attribution is recorded in `../THIRD_PARTY_NOTICES.md`; the Zstandard license includes its copyright notice.

Rust crate license files are not individually vendored here. Before distributing a binary release, collect the applicable license texts and copyright notices for the dependencies included in that release, including transitive dependencies, using its `Cargo.lock`.
