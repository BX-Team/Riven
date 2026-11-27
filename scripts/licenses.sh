#!/usr/bin/env bash
# Regenerates assets/licenses.json, the third-party licenses the About page lists.
# Needs cargo-about and jq (both in `nix develop`); run it after dependency updates.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo about generate --format json \
  | jq --rawfile lucide scripts/lucide-license.txt --rawfile asm scripts/asm-license.txt '
      [.licenses[]
        | {id, name, text,
           crates: [.used_by[].crate
             | select(.name | startswith("riven") | not)
             | "\(.name) \(.version)"] | unique}
        | select(.crates | length > 0)]
      + [{id: "ISC", name: "ISC License", text: $lucide, crates: ["Lucide icons (moonglass)"]}]
      + [{id: "BSD-3-Clause", name: "BSD 3-Clause License", text: $asm, crates: ["ASM 9.11.1 (Riven agent)"]}]
      | sort_by(-(.crates | length), .id)' \
  > assets/licenses.json
echo "assets/licenses.json: $(jq '[.[].crates[]] | unique | length' assets/licenses.json) components"
