#!/bin/sh

find ./apps/slicer-gui/ui -name \*.slint \
    | xargs slint-tr-extractor \
        -o ./apps/slicer-gui/i18n/from-slint.pot ;

find ./apps/slicer-gui/src -name '*.rs' \
    | xargs xgettext \
        -L Rust \
        --keyword='tr!:1' --keyword='tr!:1,2' \
        --from-code=UTF-8 \
        -o ./apps/slicer-gui/i18n/from-rust-without-context.pot ;

find ./apps/slicer-gui/src -name '*.rs' | xargs \
    xgettext -L Rust \
        --keyword='ctr!:1c,2' --keyword='ctr!:1c,2,3' \
        --from-code=UTF-8 \
        -o ./apps/slicer-gui/i18n/from-rust-with-context.pot ;
