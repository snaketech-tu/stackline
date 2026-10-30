# Stackline branding

Stackline is the name and identity of the app (formerly swarmdash): four rounded container blocks and a
short centered dash, paired with a bold lowercase wordmark.

## Editable masters

- [`stackline-mark.svg`](../../ui/public/brand/stackline-mark.svg) defines the
  symbol on a 96 by 80 grid. Its `stackline` group is reused directly by the app
  header, so changes to the master also update the header.
- [`stackline-wordmark.svg`](../../ui/public/brand/stackline-wordmark.svg) contains
  the wordmark as vector outlines. It uses the project's existing Plus Jakarta
  Sans ExtraBold (800), with tracking of -0.015 em. No installed or remotely
  loaded font is needed to display the exported logos.

Plus Jakarta Sans is by The Plus Jakarta Sans Project Authors and is distributed
under the SIL Open Font License 1.1. The wordmark outlines were exported from the
existing `@fontsource/plus-jakarta-sans` dependency; its font license remains in
that package.

## Generated assets

Run from the repository root after editing a master:

```sh
node dev/generate-branding.mjs
```

This updates:

- `ui/public/brand/stackline-light.svg`: teal mark and dark wordmark for the README.
- `ui/public/brand/stackline-dark.svg`: mint mark and light wordmark for dark mode.
- `ui/public/favicon.svg`: white mark on a teal rounded tile.
- `desktop/icon.svg`: the same tile at desktop export resolution.

The README selects the appropriate logo with a `picture` element. Both variants
have transparent backgrounds. The app header uses a mint mark on its dark sidebar.

The palette comes from the existing UI: teal `#0d8a86`, mint `#2fd0bd`, near-black
`#0a0f13`, and light ink `#e4ecf1`. The mark also works in one solid color.

Regenerate desktop PNG, ICO, and ICNS files with the installed Tauri CLI, from
the `desktop` directory:

```sh
npm run tauri -- icon icon.svg --output <temporary-output-directory>
npm run tauri -- icon icon.svg --png 1024 --output <temporary-output-directory>
```

Copy the generated files matching the existing filenames into `src-tauri/icons/`
and copy `1024x1024.png` to `icon.png`. The temporary output also contains mobile
assets; those are not used by this desktop project.

The [original mockup](logo-mockups/01-stackline.png) records the selected direction.
