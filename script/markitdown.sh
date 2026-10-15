#!/usr/bin/env bash
#
# markitdown.sh - convert project documents to searchable Markdown with
# MarkItDown, writing the output under doc/markitdown/.
#
# Usage:
#   script/markitdown.sh --all        convert every document in the tree
#   script/markitdown.sh <file>       convert one file (repo-relative or
#                                     absolute); prints the output path
#
# Exit status:
#   0  success
#   2  missing argument, or the requested file does not exist
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="$root_dir/doc/markitdown"
converter="$root_dir/host/tools/convert_documents.py"

mkdir -p "$output_dir"

# --all converts the whole tree.
if [[ "${1:-}" == "--all" ]]; then
  python3 "$converter" "$root_dir" "$output_dir"
  exit 0
fi

# A single file: resolve a repo-relative path against the root, then ensure it
# actually exists before converting.
source_file="${1:-}"
if [[ -z "$source_file" ]]; then
  echo "usage: $0 --all | <file>" >&2
  exit 2
fi

if [[ "$source_file" != /* ]]; then
  source_file="$root_dir/$source_file"
fi

if [[ ! -f "$source_file" ]]; then
  echo "file not found: $source_file" >&2
  exit 2
fi

python3 "$converter" "$root_dir" "$output_dir" --only "$source_file"
echo "$output_dir/${source_file#$root_dir/}.md"
