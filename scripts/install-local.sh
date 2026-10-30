#!/bin/sh
# Ставит собранный бинарь в ~/.local/bin (ext4 WSL), откуда его запускает MCP-клиент.
# Запуск прямо из target/ на /mnt/c ломается после пересборки, пока старые
# процессы держат файл: drvfs отдаёт рассинхронизированный образ и ld.so падает.
# mv поверх даёт новый inode — работающие процессы остаются на старом.
#
#   cargo build && sh scripts/install-local.sh            # debug
#   cargo build --release && sh scripts/install-local.sh release
set -eu
PROFILE=${1:-debug}
SRC="$(dirname "$0")/../target/$PROFILE/agent-memory"
DST="$HOME/.local/bin/agent-memory"
mkdir -p "$(dirname "$DST")"
cp "$SRC" "$DST.new"
chmod 755 "$DST.new"
mv -f "$DST.new" "$DST"
"$DST" --version
