#!/bin/sh
# Regenerate po/enmaster.pot from the sources and merge it into every translation.
# Rust is extracted with the C parser; keep translatable strings as plain "…" literals.
set -e
cd "$(dirname "$0")/.."
xgettext --from-code=UTF-8 --language=C --add-comments=Translators \
  --keyword=tr --keyword=trf --keyword=ntrf:1,2 \
  --package-name=enmaster --msgid-bugs-address=https://github.com/tecix/eng-master/issues \
  --files-from=po/POTFILES --output=po/enmaster.pot
for lang in $(grep -v '^#' po/LINGUAS); do
  msgmerge --quiet --update --backup=none "po/$lang.po" po/enmaster.pot
done
