# -*- mode: shell-script -*-

##
## PACKAGE TIME SCRIPT - Generates README.md for crates.io
##
## This runs during ``autogen.sh`` before ``cargo publish``. The
## generated README.md is what crates.io displays on the crate page.
##
## We append the gitchangelog-generated changelog to README.md so that
## crates.io users see release notes inline (cargo has no dedicated
## ``changelog`` field; the README is the only file crates.io renders).
##

depends pandoc

if [ -f README.org ]; then
    pandoc README.org -f org -t commonmark -o README.md.tmp || return 1

    ## Append changelog if gitchangelog is available; otherwise fall
    ## back to a static CHANGELOG.md if one happens to exist.
    if command -v gitchangelog >/dev/null 2>&1; then
        echo "" >> README.md.tmp
        echo "" >> README.md.tmp
        gitchangelog >> README.md.tmp
    elif [ -f CHANGELOG.md ]; then
        echo "" >> README.md.tmp
        echo "" >> README.md.tmp
        cat CHANGELOG.md >> README.md.tmp
    fi

    if [ -f README.md ] && diff README.md README.md.tmp > /dev/null; then
        echo "No changes in README.md" >&2
        rm README.md.tmp
    else
        echo "Updating README.md" >&2
        mv README.md.tmp README.md
    fi
fi
