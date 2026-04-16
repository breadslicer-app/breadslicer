#!/bin/sh

cd "$(dirname "$0")/.." || exit 1

# Replace target with .tmp only if content differs beyond POT-Creation-Date
maybe_update_pot() {
    target="$1"
    tmp="${target}.tmp"

    [ -f "$tmp" ] || return 0

    if [ -f "$target" ]; then
        old=$(grep -v '^"POT-Creation-Date:' "$target")
        new=$(grep -v '^"POT-Creation-Date:' "$tmp")
        if [ "$old" = "$new" ]; then
            rm -f "$tmp"
            return 0
        fi
    fi

    mv "$tmp" "$target"
}

find ./ui -name \*.slint \
    | xargs slint-tr-extractor \
        -o ./i18n/from-slint.pot.tmp ;
maybe_update_pot ./i18n/from-slint.pot

find ./src -name '*.rs' \
    | xargs xgettext \
        -L Rust \
        --keyword='tr!:1' --keyword='tr!:1,2' \
        --from-code=UTF-8 \
        -o ./i18n/from-rust-without-context.pot.tmp ;
maybe_update_pot ./i18n/from-rust-without-context.pot

find ./src -name '*.rs' \
    | xargs xgettext \
        -L Rust \
        --keyword='ctr!:1c,2' --keyword='ctr!:1c,2,3' \
        --from-code=UTF-8 \
        -o ./i18n/from-rust-with-context.pot.tmp ;
maybe_update_pot ./i18n/from-rust-with-context.pot

# Merge all .pot files into one
pot_files=""
for f in \
    ./i18n/from-slint.pot \
    ./i18n/from-rust-without-context.pot \
    ./i18n/from-rust-with-context.pot
do
    [ -f "$f" ] && pot_files="$pot_files $f"
done
msgcat --use-first $pot_files \
    -o ./i18n/sc-slicer-gui.pot.tmp ;
maybe_update_pot ./i18n/sc-slicer-gui.pot
