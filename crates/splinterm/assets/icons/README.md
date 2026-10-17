# Interface icons

`tabler-icons-subset.ttf` is a subset of the Tabler Icons outline webfont. It is
embedded in the `splinterm` executable with `include_bytes!` and drawn through
the renderer's FreeType path for agent status and Explorer disclosure. It is
never installed as a system font and never used for terminal content.

- Source package: `@tabler/icons-webfont` 3.49.0 from the npm registry,
  <https://registry.npmjs.org/@tabler/icons-webfont/-/icons-webfont-3.49.0.tgz>
  (tarball SHA-256
  `48c69c65a5f9013d482a17cab8899a09dcaec50750d1ac7324eb63a061fffb82`)
- Original file: `package/dist/fonts/tabler-icons.ttf`, SHA-256
  `d291676dc4c28b386552753bf16cf2bef0e93e5543f8e0b5c5d72c0da41ae1bc`
- Subset file SHA-256:
  `f63914ab26881e11c3b050281417597967f0dcb6aee06161c17cb1588f9b1888`
  (2536 bytes)
- Project: <https://tabler.io/icons>, <https://github.com/tabler/tabler-icons>
- License: MIT, Copyright (c) 2020-2026 Paweł Kuna; see [`LICENSE`](LICENSE).
  The notice is also carried in the repository's `THIRD_PARTY.md`, which every
  package installs under `share/licenses`.

## Subset command

Run with fontTools 4.63.0 (`nix-shell -p python3Packages.fonttools`); the output
is byte-for-byte reproducible:

```sh
pyftsubset tabler-icons.ttf \
  --unicodes=U+F98B,U+EA05,U+F226,U+EA67,U+EB0C,U+EA70,U+EA61,U+EA5F \
  --output-file=tabler-icons-subset.ttf \
  --layout-features= --no-hinting --desubroutinize --name-IDs="*" \
  --notdef-outline --drop-tables+=FFTM
```

## Codepoints

Codepoints come from the package's `dist/tabler-icons.css`. They are
Tabler-assigned codepoints, mostly private-use (U+F98B falls in the CJK
Compatibility Ideographs block), used inside this embedded face only: Splinterm places icons from trusted
interface code and never interprets these codepoints in labels, titles, agent
detail text, or terminal output.

| Icon | Codepoint | Use |
| --- | --- | --- |
| `message-question` | U+F98B | agent needs input |
| `alert-circle` | U+EA05 | agent error |
| `loader-2` | U+F226 | agent working (static, never animated) |
| `circle-check` | U+EA67 | agent done |
| `point` | U+EB0C | agent idle |
| `clock` | U+EA70 | stale agent status |
| `chevron-right` | U+EA61 | collapsed Explorer row |
| `chevron-down` | U+EA5F | expanded Explorer row |

To add an icon, add its codepoint to the command above, regenerate the subset
from the same package version, update the hashes and this table, and add the
icon to `ChromeIcon` in `crates/splinterm/src/renderer/icons.rs`.
