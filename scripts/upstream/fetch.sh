#!/usr/bin/env bash
# upstream.lock의 git 줄마다 업스트림을 고정 SHA로 받고, 받은 디렉터리를 출력한다.
set -euo pipefail

lock="$(dirname "$0")/upstream.lock"
tmp="${TMPDIR:-/tmp}"
dir="${UPSTREAM_DIR:-${tmp%/}/tokenmeter-upstream}"
mkdir -p "$dir"

while IFS=$'\t' read -r name src pin; do
  case "$src" in
    *.git) ;;
    *) continue ;; # 머리 줄, npm 줄
  esac
  if [ -d "$dir/$name/.git" ]; then
    git -C "$dir/$name" fetch --quiet origin
  else
    git clone --quiet --filter=blob:none "$src" "$dir/$name"
  fi
  git -C "$dir/$name" checkout --quiet "$pin"
done < "$lock"

echo "$dir"
