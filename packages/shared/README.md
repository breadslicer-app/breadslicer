# Shared packaging assets

Assets and metadata used by more than one packaging format, such as the
application icon, the AppStream metainfo, and the application id. Assets that
are specific to a single format live in that format's directory instead.

The metainfo here is a template: it deliberately carries no `<releases>` block.
`scripts/render-metainfo.sh` adds one when a package is built, naming the
version of the crate being packaged and the date of the build. Keeping the
release out of the tracked file is what stops it drifting, since there is no
second copy of the version left to fall behind.

Flatpak reports that release version as the package version, so it is the
number `flatpak info` shows, not just something software centres display.
