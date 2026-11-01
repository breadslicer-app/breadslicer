#!/bin/sh

cd "$(dirname "$0")/.." || exit 1

# Replace target with .tmp only if content differs beyond POT-Creation-Date
maybe_update_pot() {
    target="$1"
    tmp="${target}.tmp"

    [ -f "$tmp" ] || return 0

    if [ -f "$target" ]; then
        old=$(grep -v '^"POT-Creation-Date:' "$target" | grep -v "^#: ")
        new=$(grep -v '^"POT-Creation-Date:' "$tmp" | grep -v "^#: ")
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

find ./src ./crates -name '*.rs' \
    | xargs xgettext \
        -L Rust \
        --keyword='tr!:1' --keyword='tr!:1,2' \
        --from-code=UTF-8 \
        -o ./i18n/from-rust-without-context.pot.tmp ;
maybe_update_pot ./i18n/from-rust-without-context.pot

# The GUI's crates under ./crates translate with the same catalogs.
# A plural ctr! call has at least 4 arguments: context, singular, plural and
# count. The total-argument specs keep xgettext from reading a singular call
# whose 3rd argument is a string literal as plural. A plural call with more than
# 10 arguments needs 1 more spec.
find ./src ./crates -name '*.rs' \
    | xargs xgettext \
        -L Rust \
        --keyword='ctr!:1c,2' \
        --keyword='ctr!:1c,2,3,4t' --keyword='ctr!:1c,2,3,5t' \
        --keyword='ctr!:1c,2,3,6t' --keyword='ctr!:1c,2,3,7t' \
        --keyword='ctr!:1c,2,3,8t' --keyword='ctr!:1c,2,3,9t' \
        --keyword='ctr!:1c,2,3,10t' \
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
