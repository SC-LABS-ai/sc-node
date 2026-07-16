# SC Node Brand Assets

Original artwork for the SC Node project, created by SC LABS.

## Concept

An octagonal node frame around a central execution node. Candidate routes fan out
to the edges; the single highlighted route represents SC Node's deterministic
provider routing. Compact, technical, readable from 16 px up, and usable in
monochrome.

## Files

| File | Purpose |
|---|---|
| `sc-node-mark.svg` | Icon only, adapts to light/dark via `prefers-color-scheme` |
| `sc-node-logo.svg` | Horizontal lockup (mark + wordmark), auto light/dark |
| `sc-node-logo-light.svg` | Lockup for light backgrounds (fixed colors) |
| `sc-node-logo-dark.svg` | Lockup for dark backgrounds (fixed colors) |
| `sc-node-social-preview.svg` | 1280×640 GitHub social preview source |
| `favicon.svg` | Simplified mark for 16–32 px use |

## Colors

| Role | Light | Dark |
|---|---|---|
| Ink | `#0F172A` | `#E6EDF3` |
| Accent (route/blue) | `#1B8DEB` | `#47ADFF` |
| Preview background | — | `#0D1420` |

## Notes

- No font files are embedded or distributed; wordmarks use a system font stack
  (`Segoe UI` → system fallback). For pixel-identical rendering everywhere,
  convert text to paths before external use.
- PNG exports are optional; regenerate from the SVG sources at 512 px (mark) and
  1280×640 (social preview) when raster tooling is available.
